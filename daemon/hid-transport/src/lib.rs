// Async HID transport backed by an already-open hidraw file descriptor.
//
// The fd is received from lam-hidraw-helper via SCM_RIGHTS; this crate never
// opens /dev/hidraw* itself.  Two report types are supported:
//
//   HID_IO      — 64-byte interrupt reports, read/write via standard I/O
//   HID_FEATURE — up to 1024-byte feature reports, sent/got via ioctl
//
// Reads use tokio::io::unix::AsyncFd (epoll-backed) so that cancellation via
// tokio::time::timeout actually works.  tokio::fs::File routes I/O through the
// blocking thread pool, which keeps a background thread alive after the future
// is dropped — preventing clean shutdown.

use std::io::{self, Read, Write};
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd};
use std::time::Duration;
use tokio::io::unix::AsyncFd;

pub const REPORT_SIZE_HID_IO: usize = 64;
pub const REPORT_SIZE_HID_FEATURE_MAX: usize = 1024;

// HIDIOCSFEATURE(len) = _IOC(_IOC_WRITE|_IOC_READ, 'H', 0x06, len)
// HIDIOCGFEATURE(len) = _IOC(_IOC_WRITE|_IOC_READ, 'H', 0x07, len)
// Both directions set => use ioctl_readwrite_buf! for both.
nix::ioctl_readwrite_buf!(hid_set_feature, b'H', 0x06, u8);
nix::ioctl_readwrite_buf!(hid_get_feature, b'H', 0x07, u8);

#[derive(Debug)]
pub enum ReadError {
    Io(std::io::Error),
    Timeout,
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadError::Io(e) => write!(f, "I/O error: {e}"),
            ReadError::Timeout => write!(f, "read timed out"),
        }
    }
}

impl std::error::Error for ReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ReadError::Io(e) => Some(e),
            ReadError::Timeout => None,
        }
    }
}

impl From<std::io::Error> for ReadError {
    fn from(e: std::io::Error) -> Self {
        ReadError::Io(e)
    }
}

/// Async wrapper around an open hidraw fd received from `lam-hidraw-helper`.
pub struct HidTransport {
    inner: AsyncFd<std::fs::File>,
}

impl HidTransport {
    /// Take ownership of `fd` and wrap it in an async transport.
    /// Sets O_NONBLOCK on the fd so the epoll reactor can drive it.
    /// The fd is closed when this transport is dropped.
    pub fn from_fd(fd: OwnedFd) -> io::Result<Self> {
        // AsyncFd requires non-blocking mode.
        let flags = nix::fcntl::fcntl(&fd, nix::fcntl::FcntlArg::F_GETFL)
            .map_err(|e| io::Error::from_raw_os_error(e as i32))?;
        let flags = nix::fcntl::OFlag::from_bits_truncate(flags) | nix::fcntl::OFlag::O_NONBLOCK;
        nix::fcntl::fcntl(&fd, nix::fcntl::FcntlArg::F_SETFL(flags))
            .map_err(|e| io::Error::from_raw_os_error(e as i32))?;
        // SAFETY: fd.into_raw_fd() consumes the OwnedFd, transferring ownership to File.
        let std_file = unsafe { std::fs::File::from_raw_fd(fd.into_raw_fd()) }; // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
        Ok(Self {
            inner: AsyncFd::new(std_file)?,
        })
    }

    /// Write an interrupt (HID_IO) report. `data` must be <= `REPORT_SIZE_HID_IO`.
    pub async fn write_interrupt(&mut self, data: &[u8]) -> io::Result<()> {
        loop {
            let mut guard = self.inner.writable().await?;
            match guard.try_io(|f| f.get_ref().write(data)) {
                Ok(Ok(n)) if n == data.len() => return Ok(()),
                Ok(Ok(_)) => return Err(io::Error::new(io::ErrorKind::WriteZero, "short write")),
                Ok(Err(e)) => return Err(e),
                Err(_would_block) => continue,
            }
        }
    }

    /// Read one interrupt (HID_IO) report within `timeout`.
    /// Returns `ReadError::Timeout` if no data arrives before the deadline.
    pub async fn read_interrupt(&mut self, timeout: Duration) -> Result<Vec<u8>, ReadError> {
        let read_fut = async {
            loop {
                let mut guard = self.inner.readable().await?;
                let mut buf = vec![0u8; REPORT_SIZE_HID_IO];
                match guard.try_io(|f| f.get_ref().read(&mut buf)) {
                    Ok(Ok(n)) => {
                        buf.truncate(n);
                        return Ok::<Vec<u8>, io::Error>(buf);
                    }
                    Ok(Err(e)) => return Err(e),
                    Err(_would_block) => continue,
                }
            }
        };
        match tokio::time::timeout(timeout, read_fut).await {
            Ok(Ok(buf)) => Ok(buf),
            Ok(Err(e)) => Err(ReadError::Io(e)),
            Err(_elapsed) => Err(ReadError::Timeout),
        }
    }

    /// Send a feature (HID_FEATURE) report via `ioctl(HIDIOCSFEATURE)`.
    /// `data[0]` must be the HID report ID.
    pub fn write_feature(&self, data: &[u8]) -> io::Result<()> {
        let mut buf = data.to_vec();
        // SAFETY: ioctl on a valid hidraw fd with a correctly-sized buffer.
        unsafe { hid_set_feature(self.inner.as_raw_fd(), &mut buf) } // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
            .map(|_| ())
            .map_err(|e| io::Error::from_raw_os_error(e as i32))
    }

    /// Get a feature (HID_FEATURE) report via `ioctl(HIDIOCGFEATURE)`.
    /// `buf[0]` must be set to the desired HID report ID before calling.
    /// Returns the number of bytes filled (including the report ID byte).
    pub fn read_feature(&self, buf: &mut [u8]) -> io::Result<usize> {
        // SAFETY: ioctl on a valid hidraw fd with a correctly-sized buffer.
        unsafe { hid_get_feature(self.inner.as_raw_fd(), buf) } // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
            .map(|n| n as usize)
            .map_err(|e| io::Error::from_raw_os_error(e as i32))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nix::sys::socket::{socketpair, AddressFamily, SockFlag, SockType};
    use std::os::unix::io::OwnedFd;

    /// Transport backed by one end of a socketpair; returns the "device side" fd.
    fn make_pair() -> (HidTransport, OwnedFd) {
        let (engine_fd, peer_fd) = socketpair(
            AddressFamily::Unix,
            SockType::SeqPacket,
            None,
            SockFlag::SOCK_CLOEXEC,
        )
        .expect("socketpair");
        (HidTransport::from_fd(engine_fd).expect("from_fd"), peer_fd)
    }

    #[tokio::test]
    async fn write_interrupt_delivers_data_to_peer() {
        let (mut transport, peer_fd) = make_pair();

        transport.write_interrupt(b"hello world").await.unwrap();

        let mut buf = [0u8; 11];
        nix::unistd::read(&peer_fd, &mut buf).unwrap();
        assert_eq!(&buf, b"hello world");
    }

    #[tokio::test]
    async fn read_interrupt_returns_data_from_peer() {
        let (mut transport, peer_fd) = make_pair();

        let payload = [0x42u8; 16];
        nix::unistd::write(&peer_fd, &payload).unwrap();

        let result = transport
            .read_interrupt(Duration::from_millis(500))
            .await
            .unwrap();
        assert_eq!(result, payload);
    }

    #[tokio::test]
    async fn read_interrupt_times_out_with_no_data() {
        let (mut transport, _peer) = make_pair();
        // _peer kept alive so the socket stays connected (not EOF).

        let err = transport
            .read_interrupt(Duration::from_millis(50))
            .await
            .unwrap_err();
        assert!(matches!(err, ReadError::Timeout));
    }

    #[tokio::test]
    async fn write_then_read_roundtrip() {
        let (mut t1, peer_fd) = make_pair();
        let mut t2 = HidTransport::from_fd(peer_fd).expect("from_fd");

        let sent = [0u8, 1, 2, 3, 4, 5, 6, 7];
        t1.write_interrupt(&sent).await.unwrap();

        let received = t2.read_interrupt(Duration::from_millis(500)).await.unwrap();
        assert_eq!(received, sent);
    }
}
