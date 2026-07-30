use std::io::IoSlice;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::{AsRawFd, RawFd};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tracing::{error, info, warn};

const STEELSERIES_VID: u16 = 0x1038;

// Protocol
// Engine → Helper : <hidraw_path>\n   (max 255 bytes, newline-terminated)
// Helper → Engine : 0x00              (rejected — any check failed)
//                   0x01 + SCM_RIGHTS (accepted — fd attached as ancillary data)

pub struct HelperConfig {
    pub sysfs_base: PathBuf,
    /// Root of the device filesystem; `/dev` in production, a temp dir in tests.
    pub dev_root: PathBuf,
}

impl HelperConfig {
    pub fn system() -> Self {
        Self {
            sysfs_base: PathBuf::from("/sys"),
            dev_root: PathBuf::from("/dev"),
        }
    }
}

pub fn bind_socket(path: &Path) -> std::io::Result<UnixListener> {
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    Ok(listener)
}

pub async fn serve(listener: UnixListener, config: Arc<HelperConfig>) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let cfg = Arc::clone(&config);
                if let Err(e) = handle_connection(stream, &cfg).await {
                    error!("connection error: {e}");
                }
            }
            Err(e) => {
                error!("accept failed: {e}");
            }
        }
    }
}

async fn handle_connection(mut stream: UnixStream, config: &HelperConfig) -> std::io::Result<()> {
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

    let vid = match read_vid(&config.sysfs_base, hidraw_name) {
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

    let hid_path = match hid_device_path(&config.sysfs_base, hidraw_name) {
        Ok(p) => p,
        Err(e) => {
            warn!("cannot resolve HID device path for {hidraw_name}: {e}");
            stream.write_all(&[0x00]).await?;
            return Ok(());
        }
    };

    if !has_audio_interface(&hid_path).unwrap_or(false) {
        warn!("rejected {hidraw_name}: no USB audio interface on parent device");
        stream.write_all(&[0x00]).await?;
        return Ok(());
    }

    let device_path = config.dev_root.join(hidraw_name);
    let hidraw_file = match open_hidraw(&device_path) {
        Ok(f) => f,
        Err(e) => {
            warn!("cannot open {}: {e}", device_path.display());
            stream.write_all(&[0x00]).await?;
            return Ok(());
        }
    };

    if let Err(e) = send_fd(&stream, hidraw_file.as_raw_fd()) {
        warn!("sendmsg failed for {hidraw_name}: {e}");
        return Err(e);
    }

    info!("fd for {hidraw_name} passed to engine (VID {vid:#06x})");
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

fn hid_device_path(sysfs_base: &Path, hidraw_name: &str) -> std::io::Result<PathBuf> {
    let symlink = sysfs_base
        .join("class/hidraw")
        .join(hidraw_name)
        .join("device");
    std::fs::canonicalize(symlink)
}

// Checks whether the USB device that owns the HID function has at least one
// interface with bInterfaceClass == 01 (USB Audio). Keyboards and mice are
// pure HID devices and do not have an audio interface; headsets do.
pub fn has_audio_interface(hid_dev: &Path) -> std::io::Result<bool> {
    let usb_dev = hid_dev.parent().and_then(Path::parent).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "unexpected sysfs layout")
    })?;
    for entry in std::fs::read_dir(usb_dev)? {
        let class_file = entry?.path().join("bInterfaceClass");
        if let Ok(class) = std::fs::read_to_string(class_file) {
            if class.trim() == "01" {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn open_hidraw(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
}

// Sends `fd` as SCM_RIGHTS ancillary data, using the acceptance byte 0x01 as
// the required data payload. The caller must drop `fd` after this returns; the
// kernel duplicates it into the receiving process during sendmsg.
pub fn send_fd(socket: &UnixStream, fd: RawFd) -> std::io::Result<()> {
    let data = [0x01u8];
    let iov = [IoSlice::new(&data)];
    let cmsg = [nix::sys::socket::ControlMessage::ScmRights(&[fd])];
    nix::sys::socket::sendmsg::<()>(
        socket.as_raw_fd(),
        &iov,
        &cmsg,
        nix::sys::socket::MsgFlags::empty(),
        None,
    )
    .map(|_| ())
    .map_err(|e| std::io::Error::from_raw_os_error(e as i32))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{IoSliceMut, Read};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::io::FromRawFd;
    use tempfile::TempDir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::UnixStream;

    fn fake_sysfs_simple(dir: &TempDir, hidraw_name: &str, vid: u16) {
        let dev_dir = dir
            .path()
            .join("class/hidraw")
            .join(hidraw_name)
            .join("device");
        std::fs::create_dir_all(&dev_dir).unwrap();
        std::fs::write(dev_dir.join("idVendor"), format!("{vid:#06x}\n")).unwrap();
    }

    // Full fake sysfs: symlinked device tree + audio interface sibling.
    fn fake_sysfs_headset(dir: &TempDir, hidraw_name: &str, vid: u16) {
        let hid_fn = dir.path().join("usb_dev/hid_iface/hid_fn");
        std::fs::create_dir_all(&hid_fn).unwrap();
        std::fs::write(hid_fn.join("idVendor"), format!("{vid:#06x}\n")).unwrap();

        let audio_iface = dir.path().join("usb_dev/audio_iface");
        std::fs::create_dir_all(&audio_iface).unwrap();
        std::fs::write(audio_iface.join("bInterfaceClass"), "01\n").unwrap();

        let hidraw_dir = dir.path().join("class/hidraw").join(hidraw_name);
        std::fs::create_dir_all(&hidraw_dir).unwrap();
        std::os::unix::fs::symlink(&hid_fn, hidraw_dir.join("device")).unwrap();
    }

    fn fake_sysfs_keyboard(dir: &TempDir, hidraw_name: &str, vid: u16) {
        let hid_fn = dir.path().join("usb_dev/hid_iface/hid_fn");
        std::fs::create_dir_all(&hid_fn).unwrap();
        std::fs::write(hid_fn.join("idVendor"), format!("{vid:#06x}\n")).unwrap();

        let hidraw_dir = dir.path().join("class/hidraw").join(hidraw_name);
        std::fs::create_dir_all(&hidraw_dir).unwrap();
        std::os::unix::fs::symlink(&hid_fn, hidraw_dir.join("device")).unwrap();
    }

    // Creates a writable fake device file at dev_dir/hidraw_name.
    fn fake_device(dev_dir: &TempDir, hidraw_name: &str) {
        std::fs::write(dev_dir.path().join(hidraw_name), b"").unwrap();
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
        assert!(!vid_is_allowed(0x046d));
        assert!(!vid_is_allowed(0x045e));
        assert!(!vid_is_allowed(0x0000));
    }

    #[test]
    fn read_vid_parses_hex_with_prefix() {
        let dir = TempDir::new().unwrap();
        fake_sysfs_simple(&dir, "hidraw0", 0x1038);
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

    #[test]
    fn headset_with_audio_interface_is_detected() {
        let dir = TempDir::new().unwrap();
        fake_sysfs_headset(&dir, "hidraw0", 0x1038);
        let hid_fn = dir.path().join("usb_dev/hid_iface/hid_fn");
        assert!(has_audio_interface(&hid_fn).unwrap());
    }

    #[test]
    fn keyboard_without_audio_interface_is_not_detected() {
        let dir = TempDir::new().unwrap();
        fake_sysfs_keyboard(&dir, "hidraw0", 0x1038);
        let hid_fn = dir.path().join("usb_dev/hid_iface/hid_fn");
        assert!(!has_audio_interface(&hid_fn).unwrap());
    }

    // --- fd passing ---

    #[tokio::test]
    async fn fd_is_transferred_over_unix_socket() {
        use nix::sys::socket::{ControlMessageOwned, MsgFlags};

        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), b"hello").unwrap();

        let (sender, receiver) = tokio::net::UnixStream::pair().unwrap();

        let src = std::fs::File::open(tmp.path()).unwrap();
        send_fd(&sender, src.as_raw_fd()).unwrap();
        drop(src);

        // iov holds &mut data_buf; keep them in an inner block so the borrow
        // is released before we assert on data_buf and use received_fd.
        let mut data_buf = [0u8; 1];
        let received_fd = {
            let mut iov = [IoSliceMut::new(&mut data_buf)];
            let mut cmsg_buf = nix::cmsg_space!(RawFd);
            let msg = nix::sys::socket::recvmsg::<()>(
                receiver.as_raw_fd(),
                &mut iov,
                Some(&mut cmsg_buf),
                MsgFlags::empty(),
            )
            .unwrap();
            msg.cmsgs()
                .unwrap()
                .find_map(|cmsg| {
                    if let ControlMessageOwned::ScmRights(fds) = cmsg {
                        fds.first().copied()
                    } else {
                        None
                    }
                })
                .expect("no fd received")
        }; // iov and msg drop here, releasing &mut data_buf

        assert_eq!(data_buf[0], 0x01);

        let mut received_file = unsafe { std::fs::File::from_raw_fd(received_fd) };
        let mut content = String::new();
        received_file.read_to_string(&mut content).unwrap();
        assert_eq!(content, "hello");
    }

    #[test]
    fn open_hidraw_succeeds_on_existing_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("hidraw0");
        std::fs::write(&path, b"").unwrap();
        assert!(open_hidraw(&path).is_ok());
    }

    #[test]
    fn open_hidraw_fails_on_missing_file() {
        let dir = TempDir::new().unwrap();
        assert!(open_hidraw(&dir.path().join("hidraw99")).is_err());
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

    // --- full-stack integration ---

    async fn start_server(sock_dir: &TempDir, sysfs_dir: &TempDir, dev_dir: &TempDir) -> PathBuf {
        let sock_path = sock_dir.path().join("helper.sock");
        let listener = bind_socket(&sock_path).unwrap();
        let config = Arc::new(HelperConfig {
            sysfs_base: sysfs_dir.path().to_path_buf(),
            dev_root: dev_dir.path().to_path_buf(),
        });
        tokio::spawn(async move { serve(listener, config).await });
        sock_path
    }

    #[tokio::test]
    async fn server_accepts_steelseries_headset_and_passes_fd() {
        let sock_dir = TempDir::new().unwrap();
        let sysfs_dir = TempDir::new().unwrap();
        let dev_dir = TempDir::new().unwrap();
        fake_sysfs_headset(&sysfs_dir, "hidraw0", 0x1038);
        fake_device(&dev_dir, "hidraw0");
        let sock_path = start_server(&sock_dir, &sysfs_dir, &dev_dir).await;

        let mut client = UnixStream::connect(&sock_path).await.unwrap();
        client.write_all(b"/dev/hidraw0\n").await.unwrap();

        // Read the acceptance byte from the data portion of sendmsg.
        let mut resp = [0u8; 1];
        client.read_exact(&mut resp).await.unwrap();
        assert_eq!(resp[0], 0x01);
    }

    #[tokio::test]
    async fn server_rejects_unknown_vid() {
        let sock_dir = TempDir::new().unwrap();
        let sysfs_dir = TempDir::new().unwrap();
        let dev_dir = TempDir::new().unwrap();
        fake_sysfs_headset(&sysfs_dir, "hidraw0", 0x046d);
        let sock_path = start_server(&sock_dir, &sysfs_dir, &dev_dir).await;

        let mut client = UnixStream::connect(&sock_path).await.unwrap();
        client.write_all(b"/dev/hidraw0\n").await.unwrap();
        let mut resp = [0u8; 1];
        client.read_exact(&mut resp).await.unwrap();
        assert_eq!(resp[0], 0x00);
    }

    #[tokio::test]
    async fn server_rejects_steelseries_keyboard() {
        let sock_dir = TempDir::new().unwrap();
        let sysfs_dir = TempDir::new().unwrap();
        let dev_dir = TempDir::new().unwrap();
        fake_sysfs_keyboard(&sysfs_dir, "hidraw0", 0x1038);
        let sock_path = start_server(&sock_dir, &sysfs_dir, &dev_dir).await;

        let mut client = UnixStream::connect(&sock_path).await.unwrap();
        client.write_all(b"/dev/hidraw0\n").await.unwrap();
        let mut resp = [0u8; 1];
        client.read_exact(&mut resp).await.unwrap();
        assert_eq!(resp[0], 0x00);
    }

    #[tokio::test]
    async fn server_rejects_non_hidraw_path() {
        let sock_dir = TempDir::new().unwrap();
        let sysfs_dir = TempDir::new().unwrap();
        let dev_dir = TempDir::new().unwrap();
        let sock_path = start_server(&sock_dir, &sysfs_dir, &dev_dir).await;

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
