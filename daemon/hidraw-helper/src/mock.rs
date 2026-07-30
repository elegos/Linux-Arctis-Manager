// In-process mock for lam-hidraw-helper. Speaks the same Unix socket protocol
// as the real helper but, instead of opening /dev/hidraw*, creates a socketpair
// and passes one end to the requesting engine. The other end (the "device side")
// is forwarded to the test harness via an mpsc channel.
//
// Usage in tests:
//   let mut rx = mock::start_mock(&sock_path);
//   // connect as engine, send /dev/hidraw0\n, receive 0x01 + fd via SCM_RIGHTS
//   let device_fd = rx.recv().await.unwrap(); // harness side of the socketpair

use nix::sys::socket::{socketpair, AddressFamily, SockFlag, SockType};
use std::os::unix::io::{AsRawFd, OwnedFd};
use std::path::Path;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;

/// Bind mock socket at `sock_path` and start serving in a background task.
/// Returns a receiver that yields the device-side `OwnedFd` for each accepted
/// engine request. The engine-side fd is delivered via SCM_RIGHTS automatically.
pub fn start_mock(sock_path: &Path) -> mpsc::Receiver<OwnedFd> {
    let listener = crate::server::bind_socket(sock_path).expect("bind mock socket");
    let (tx, rx) = mpsc::channel(8);
    tokio::spawn(serve_mock(listener, tx));
    rx
}

pub async fn serve_mock(listener: UnixListener, harness_tx: mpsc::Sender<OwnedFd>) {
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        let tx = harness_tx.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_mock_conn(stream, tx).await {
                eprintln!("mock: {e}");
            }
        });
    }
}

async fn handle_mock_conn(
    mut stream: UnixStream,
    harness_tx: mpsc::Sender<OwnedFd>,
) -> std::io::Result<()> {
    let own_uid = nix::unistd::getuid().as_raw();
    let peer_uid = stream.peer_cred()?.uid();
    if peer_uid != own_uid {
        stream.write_all(&[0x00]).await?;
        return Ok(());
    }

    let mut buf = [0u8; 256];
    let n = stream.read(&mut buf).await?;
    if n == 0 {
        return Ok(());
    }
    let raw = std::str::from_utf8(&buf[..n])
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let dev_path = raw.trim_end_matches(['\n', '\r', '\0']);
    if !dev_path.starts_with("/dev/hidraw") {
        stream.write_all(&[0x00]).await?;
        return Ok(());
    }

    let (engine_fd, harness_fd) = socketpair(
        AddressFamily::Unix,
        SockType::Stream,
        None,
        SockFlag::SOCK_CLOEXEC,
    )
    .map_err(|e| std::io::Error::from_raw_os_error(e as i32))?;

    crate::server::send_fd(&stream, engine_fd.as_raw_fd())?;

    let _ = harness_tx.send(harness_fd).await;

    Ok(())
    // engine_fd is dropped here; the kernel copy already lives in the engine process
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{IoSliceMut, Read, Write};
    use std::os::unix::io::{FromRawFd, RawFd};
    use tempfile::TempDir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::UnixStream;

    // Blocking recvmsg helper — safe in tests because the sender has already
    // called sendmsg before we call this, so it never actually blocks.
    fn recv_accepted_fd(stream: &UnixStream) -> (u8, RawFd) {
        use nix::sys::socket::{ControlMessageOwned, MsgFlags};
        let mut data_buf = [0u8; 1];
        let fd = {
            let mut iov = [IoSliceMut::new(&mut data_buf)];
            let mut cmsg_buf = nix::cmsg_space!(RawFd);
            let msg = nix::sys::socket::recvmsg::<()>(
                stream.as_raw_fd(),
                &mut iov,
                Some(&mut cmsg_buf),
                MsgFlags::empty(),
            )
            .unwrap();
            msg.cmsgs()
                .unwrap()
                .find_map(|c| {
                    if let ControlMessageOwned::ScmRights(fds) = c {
                        fds.first().copied()
                    } else {
                        None
                    }
                })
                .expect("no SCM_RIGHTS fd in message")
        };
        (data_buf[0], fd)
    }

    #[tokio::test]
    async fn mock_accepts_hidraw_path_and_passes_fd() {
        let dir = TempDir::new().unwrap();
        let sock_path = dir.path().join("mock.sock");
        let mut rx = start_mock(&sock_path);

        let mut client = UnixStream::connect(&sock_path).await.unwrap();
        client.write_all(b"/dev/hidraw0\n").await.unwrap();

        // Engine receives 0x01 acceptance byte + fd via SCM_RIGHTS.
        // Wait for the mock to respond before calling blocking recvmsg.
        client.readable().await.unwrap();
        let (byte, engine_fd) = recv_accepted_fd(&client);
        assert_eq!(byte, 0x01);

        // Harness gets the other end of the socketpair.
        let harness_fd = rx.recv().await.expect("harness fd");

        // Write from harness side → read from engine side to confirm the pair
        // is connected and bidirectional I/O works.
        nix::unistd::write(&harness_fd, b"ping").unwrap();

        let mut engine_file = unsafe { std::fs::File::from_raw_fd(engine_fd) };
        let mut resp = [0u8; 4];
        engine_file.read_exact(&mut resp).unwrap();
        assert_eq!(&resp, b"ping");

        // Write from engine side → read from harness side.
        engine_file.write_all(b"pong").unwrap();
        let mut pong = [0u8; 4];
        nix::unistd::read(&harness_fd, &mut pong).unwrap();
        assert_eq!(&pong, b"pong");
    }

    #[tokio::test]
    async fn mock_rejects_non_hidraw_path() {
        let dir = TempDir::new().unwrap();
        let sock_path = dir.path().join("mock.sock");
        let _rx = start_mock(&sock_path);

        let mut client = UnixStream::connect(&sock_path).await.unwrap();
        client.write_all(b"/dev/input/event0\n").await.unwrap();

        let mut resp = [0u8; 1];
        client.read_exact(&mut resp).await.unwrap();
        assert_eq!(resp[0], 0x00);
    }

    #[tokio::test]
    async fn mock_serves_multiple_requests_independently() {
        let dir = TempDir::new().unwrap();
        let sock_path = dir.path().join("mock.sock");
        let mut rx = start_mock(&sock_path);

        for i in 0u8..3 {
            let mut client = UnixStream::connect(&sock_path).await.unwrap();
            client
                .write_all(format!("/dev/hidraw{i}\n").as_bytes())
                .await
                .unwrap();

            client.readable().await.unwrap();
            let (byte, engine_fd) = recv_accepted_fd(&client);
            assert_eq!(byte, 0x01);

            let harness_fd = rx.recv().await.expect("harness fd");

            // Tag each pair with the index to prove they're distinct.
            let tag = [i];
            nix::unistd::write(&harness_fd, &tag).unwrap();
            let mut got = [0u8; 1];
            let mut f = unsafe { std::fs::File::from_raw_fd(engine_fd) };
            f.read_exact(&mut got).unwrap();
            assert_eq!(got[0], i);
        }
    }
}
