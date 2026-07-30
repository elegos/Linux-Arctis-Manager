use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use tokio::net::{UnixListener, UnixStream};
use tracing::{error, info, warn};

pub fn bind_socket(path: &Path) -> std::io::Result<UnixListener> {
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    Ok(listener)
}

pub async fn serve(listener: UnixListener) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                if let Err(e) = handle_connection(stream).await {
                    error!("connection error: {e}");
                }
            }
            Err(e) => {
                error!("accept failed: {e}");
            }
        }
    }
}

async fn handle_connection(stream: UnixStream) -> std::io::Result<()> {
    let own_uid = nix::unistd::getuid().as_raw();
    let peer_uid = stream.peer_cred()?.uid();

    if !uid_is_authorized(peer_uid, own_uid) {
        warn!("rejected connection from UID {peer_uid} (own UID: {own_uid})");
        return Ok(());
    }

    info!("authorized connection from UID {peer_uid}");

    // VID allowlist enforcement  — E2-S3
    // file descriptor passing    — E2-S4
    Ok(())
}

fn uid_is_authorized(peer_uid: u32, own_uid: u32) -> bool {
    peer_uid == own_uid
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;
    use tokio::net::UnixStream;

    // pure logic

    #[test]
    fn same_uid_is_authorized() {
        assert!(uid_is_authorized(1000, 1000));
    }

    #[test]
    fn different_uid_is_rejected() {
        assert!(!uid_is_authorized(1001, 1000));
    }

    #[test]
    fn root_uid_is_rejected_when_own_is_unprivileged() {
        assert!(!uid_is_authorized(0, 1000));
    }

    // socket setup (carried over from E2-S1)

    #[tokio::test]
    async fn socket_created_at_path() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("helper.sock");
        let _listener = bind_socket(&path).unwrap();
        assert!(path.exists());
    }

    #[tokio::test]
    async fn socket_permissions_are_700() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("helper.sock");
        let _listener = bind_socket(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }

    #[tokio::test]
    async fn stale_socket_file_is_replaced() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("helper.sock");
        std::fs::write(&path, b"stale").unwrap();
        let _listener = bind_socket(&path).unwrap();
        assert!(path.exists());
    }

    #[tokio::test]
    async fn client_can_connect() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("helper.sock");
        let listener = bind_socket(&path).unwrap();
        let path_clone = path.clone();

        let server = tokio::spawn(async move { listener.accept().await.unwrap() });
        let _client = UnixStream::connect(&path_clone).await.unwrap();
        server.await.unwrap();
    }

    // peer credentials

    #[tokio::test]
    async fn peer_uid_matches_own_uid_for_same_process() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("helper.sock");
        let listener = bind_socket(&path).unwrap();
        let path_clone = path.clone();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let own_uid = nix::unistd::getuid().as_raw();
            let peer_uid = stream.peer_cred().unwrap().uid();
            assert_eq!(peer_uid, own_uid);
        });

        let _client = UnixStream::connect(&path_clone).await.unwrap();
        server.await.unwrap();
    }
}
