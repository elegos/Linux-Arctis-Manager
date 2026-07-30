use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use tokio::net::{UnixListener, UnixStream};
use tracing::{error, info};

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
                info!("connection accepted");
                handle_connection(stream).await;
            }
            Err(e) => {
                error!("accept failed: {e}");
            }
        }
    }
}

async fn handle_connection(_stream: UnixStream) {
    // peer credential validation  — E2-S2
    // VID allowlist enforcement   — E2-S3
    // file descriptor passing     — E2-S4
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;
    use tokio::net::UnixStream;

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
}
