use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tracing::{error, info, warn};

const STEELSERIES_VID: u16 = 0x1038;

// Protocol
// Engine → Helper : <hidraw_path>\n   (max 255 bytes, newline-terminated)
// Helper → Engine : 0x00              (rejected — any check failed)
//                   0x01              (accepted — fd follows via SCM_RIGHTS, E2-S4)

pub fn bind_socket(path: &Path) -> std::io::Result<UnixListener> {
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    Ok(listener)
}

pub async fn serve(listener: UnixListener, sysfs_base: Arc<PathBuf>) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let sysfs = Arc::clone(&sysfs_base);
                if let Err(e) = handle_connection(stream, &sysfs).await {
                    error!("connection error: {e}");
                }
            }
            Err(e) => {
                error!("accept failed: {e}");
            }
        }
    }
}

async fn handle_connection(mut stream: UnixStream, sysfs_base: &Path) -> std::io::Result<()> {
    let own_uid = nix::unistd::getuid().as_raw();
    let peer_uid = stream.peer_cred()?.uid();

    if !uid_is_authorized(peer_uid, own_uid) {
        warn!("rejected connection from UID {peer_uid} (own UID: {own_uid})");
        return Ok(());
    }

    let mut buf = [0u8; 256];
    let n = stream.read(&mut buf).await?;
    if n == 0 {
        warn!("empty request from UID {peer_uid}");
        return Ok(());
    }
    let raw = std::str::from_utf8(&buf[..n])
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let dev_path = raw.trim_end_matches(['\n', '\r', '\0']);

    let hidraw_name = match hidraw_name_from_path(dev_path) {
        Some(n) => n,
        None => {
            warn!("rejected path: {dev_path:?}");
            stream.write_all(&[0x00]).await?;
            return Ok(());
        }
    };

    let vid = match read_vid(sysfs_base, hidraw_name) {
        Ok(v) => v,
        Err(e) => {
            warn!("cannot read VID for {hidraw_name}: {e}");
            stream.write_all(&[0x00]).await?;
            return Ok(());
        }
    };

    if !vid_is_allowed(vid) {
        warn!("rejected device {hidraw_name}: VID {vid:#06x} not in allowlist");
        stream.write_all(&[0x00]).await?;
        return Ok(());
    }

    info!("VID {vid:#06x} allowed for {hidraw_name}");
    stream.write_all(&[0x01]).await?;

    // file descriptor passing — E2-S4
    Ok(())
}

fn uid_is_authorized(peer_uid: u32, own_uid: u32) -> bool {
    peer_uid == own_uid
}

fn hidraw_name_from_path(dev_path: &str) -> Option<&str> {
    if !dev_path.starts_with("/dev/hidraw") {
        return None;
    }
    Path::new(dev_path).file_name()?.to_str()
}

pub fn read_vid(sysfs_base: &Path, hidraw_name: &str) -> std::io::Result<u16> {
    let vid_path = sysfs_base
        .join("class/hidraw")
        .join(hidraw_name)
        .join("device/idVendor");
    let raw = std::fs::read_to_string(vid_path)?;
    let hex = raw.trim().trim_start_matches("0x");
    u16::from_str_radix(hex, 16)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

fn vid_is_allowed(vid: u16) -> bool {
    vid == STEELSERIES_VID
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;
    use tokio::io::AsyncReadExt;
    use tokio::net::UnixStream;

    fn fake_sysfs(dir: &TempDir, hidraw_name: &str, vid: u16) {
        let dev_dir = dir
            .path()
            .join("class/hidraw")
            .join(hidraw_name)
            .join("device");
        std::fs::create_dir_all(&dev_dir).unwrap();
        std::fs::write(dev_dir.join("idVendor"), format!("{vid:#06x}\n")).unwrap();
    }

    // --- pure logic ---

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

    #[test]
    fn hidraw_path_extracts_device_name() {
        assert_eq!(hidraw_name_from_path("/dev/hidraw0"), Some("hidraw0"));
        assert_eq!(hidraw_name_from_path("/dev/hidraw12"), Some("hidraw12"));
    }

    #[test]
    fn non_hidraw_path_is_rejected() {
        assert_eq!(hidraw_name_from_path("/dev/input/event0"), None);
        assert_eq!(hidraw_name_from_path("/dev/sda"), None);
        assert_eq!(hidraw_name_from_path("hidraw0"), None);
        assert_eq!(hidraw_name_from_path(""), None);
    }

    #[test]
    fn steelseries_vid_is_allowed() {
        assert!(vid_is_allowed(0x1038));
    }

    #[test]
    fn other_vids_are_rejected() {
        assert!(!vid_is_allowed(0x046d)); // Logitech
        assert!(!vid_is_allowed(0x045e)); // Microsoft
        assert!(!vid_is_allowed(0x0000));
    }

    #[test]
    fn read_vid_parses_hex_with_prefix() {
        let dir = TempDir::new().unwrap();
        fake_sysfs(&dir, "hidraw0", 0x1038);
        assert_eq!(read_vid(dir.path(), "hidraw0").unwrap(), 0x1038);
    }

    #[test]
    fn read_vid_parses_hex_without_prefix() {
        let dir = TempDir::new().unwrap();
        let dev_dir = dir.path().join("class/hidraw/hidraw0/device");
        std::fs::create_dir_all(&dev_dir).unwrap();
        std::fs::write(dev_dir.join("idVendor"), "1038\n").unwrap();
        assert_eq!(read_vid(dir.path(), "hidraw0").unwrap(), 0x1038);
    }

    #[test]
    fn read_vid_errors_on_missing_device() {
        let dir = TempDir::new().unwrap();
        assert!(read_vid(dir.path(), "hidraw99").is_err());
    }

    #[test]
    fn read_vid_errors_on_invalid_content() {
        let dir = TempDir::new().unwrap();
        let dev_dir = dir.path().join("class/hidraw/hidraw0/device");
        std::fs::create_dir_all(&dev_dir).unwrap();
        std::fs::write(dev_dir.join("idVendor"), "not-a-number\n").unwrap();
        assert!(read_vid(dir.path(), "hidraw0").is_err());
    }

    // --- socket setup ---

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

    // --- VID allowlist integration ---

    async fn start_server(sock_dir: &TempDir, sysfs_dir: &TempDir) -> PathBuf {
        let sock_path = sock_dir.path().join("helper.sock");
        let listener = bind_socket(&sock_path).unwrap();
        let sysfs = Arc::new(sysfs_dir.path().to_path_buf());
        tokio::spawn(async move { serve(listener, sysfs).await });
        sock_path
    }

    #[tokio::test]
    async fn server_rejects_unknown_vid() {
        let sock_dir = TempDir::new().unwrap();
        let sysfs_dir = TempDir::new().unwrap();
        fake_sysfs(&sysfs_dir, "hidraw0", 0x046d); // Logitech
        let sock_path = start_server(&sock_dir, &sysfs_dir).await;

        let mut client = UnixStream::connect(&sock_path).await.unwrap();
        client.write_all(b"/dev/hidraw0\n").await.unwrap();

        let mut resp = [0u8; 1];
        client.read_exact(&mut resp).await.unwrap();
        assert_eq!(resp[0], 0x00);
    }

    #[tokio::test]
    async fn server_accepts_steelseries_vid() {
        let sock_dir = TempDir::new().unwrap();
        let sysfs_dir = TempDir::new().unwrap();
        fake_sysfs(&sysfs_dir, "hidraw0", 0x1038);
        let sock_path = start_server(&sock_dir, &sysfs_dir).await;

        let mut client = UnixStream::connect(&sock_path).await.unwrap();
        client.write_all(b"/dev/hidraw0\n").await.unwrap();

        let mut resp = [0u8; 1];
        client.read_exact(&mut resp).await.unwrap();
        assert_eq!(resp[0], 0x01);
    }

    #[tokio::test]
    async fn server_rejects_non_hidraw_path() {
        let sock_dir = TempDir::new().unwrap();
        let sysfs_dir = TempDir::new().unwrap();
        let sock_path = start_server(&sock_dir, &sysfs_dir).await;

        let mut client = UnixStream::connect(&sock_path).await.unwrap();
        client.write_all(b"/dev/input/event0\n").await.unwrap();

        let mut resp = [0u8; 1];
        client.read_exact(&mut resp).await.unwrap();
        assert_eq!(resp[0], 0x00);
    }

    #[tokio::test]
    async fn peer_uid_matches_own_uid_for_same_process() {
        let sock_dir = TempDir::new().unwrap();
        let path = sock_dir.path().join("helper.sock");
        let listener = bind_socket(&path).unwrap();
        let path_clone = path.clone();

        let server = tokio::spawn(async move { listener.accept().await.unwrap() });
        let _client = UnixStream::connect(&path_clone).await.unwrap();
        let (stream, _) = server.await.unwrap();
        let own_uid = nix::unistd::getuid().as_raw();
        assert_eq!(stream.peer_cred().unwrap().uid(), own_uid);
    }
}
