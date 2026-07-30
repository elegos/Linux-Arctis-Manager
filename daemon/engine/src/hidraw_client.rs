// Client side of the lam-hidraw-helper Unix-socket protocol.
//
// The engine sends a hidraw path terminated with `\n`; the helper authenticates
// by peer credentials, opens the node, and responds with one status byte (0x01
// = accepted) plus the open fd delivered via SCM_RIGHTS in the same message.

use std::io::{self, IoSliceMut};
use std::os::unix::io::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::Path;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;

/// Connect to the running `lam-hidraw-helper` at `sock_path` and ask it to
/// open `hidraw_path`.  Returns the received `OwnedFd` on success.
pub async fn request_fd(sock_path: &Path, hidraw_path: &str) -> io::Result<OwnedFd> {
    let mut stream = UnixStream::connect(sock_path).await?;
    stream
        .write_all(format!("{hidraw_path}\n").as_bytes())
        .await?;

    // The helper replies with one data byte + the fd in SCM_RIGHTS.
    stream.readable().await?;
    let (status, fd_opt) = recv_scm_rights(&stream)?;

    if status != 0x01 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("hidraw-helper rejected '{hidraw_path}' (status={status:#04x})"),
        ));
    }

    fd_opt.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "hidraw-helper accepted but sent no fd",
        )
    })
}

/// Receive one byte of data and an optional `SCM_RIGHTS` fd from `stream`.
fn recv_scm_rights(stream: &UnixStream) -> io::Result<(u8, Option<OwnedFd>)> {
    use nix::sys::socket::{ControlMessageOwned, MsgFlags};

    let mut data_buf = [0u8; 1];
    let raw_fd = {
        let mut iov = [IoSliceMut::new(&mut data_buf)];
        let mut cmsg_buf = nix::cmsg_space!(RawFd);
        let msg = nix::sys::socket::recvmsg::<()>(
            stream.as_raw_fd(),
            &mut iov,
            Some(&mut cmsg_buf),
            MsgFlags::empty(),
        )
        .map_err(|e| io::Error::from_raw_os_error(e as i32))?;

        msg.cmsgs().unwrap().find_map(|c| {
            if let ControlMessageOwned::ScmRights(fds) = c {
                fds.first().copied()
            } else {
                None
            }
        })
    };

    let owned = raw_fd.map(|fd| unsafe { OwnedFd::from_raw_fd(fd) });
    Ok((data_buf[0], owned))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lam_hidraw_helper::mock;
    use tempfile::TempDir;

    #[tokio::test]
    async fn request_fd_returns_valid_fd_for_hidraw_path() {
        let dir = TempDir::new().unwrap();
        let sock_path = dir.path().join("helper.sock");
        let _rx = mock::start_mock(&sock_path);

        let fd = request_fd(&sock_path, "/dev/hidraw0")
            .await
            .expect("should receive fd from mock");

        assert!(fd.as_raw_fd() >= 0);
    }

    #[tokio::test]
    async fn request_fd_returns_permission_denied_for_non_hidraw() {
        let dir = TempDir::new().unwrap();
        let sock_path = dir.path().join("helper.sock");
        let _rx = mock::start_mock(&sock_path);

        let err = request_fd(&sock_path, "/dev/input/event0")
            .await
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
    }
}
