// lam-hidraw-helper-mock: test double for lam-hidraw-helper.
//
// Speaks the same Unix socket protocol as the real helper but creates a
// socketpair instead of opening /dev/hidraw*. The "device side" fd is forwarded
// to a test harness via a second socket (LAM_HARNESS_SOCKET).
//
// Environment variables:
//   LAM_HELPER_SOCKET  — where to listen for engine connections (required)
//   LAM_HARNESS_SOCKET — where the test harness connects to receive device-side
//                        fds (defaults to LAM_HELPER_SOCKET + ".harness")
//
// Protocol with the harness:
//   Harness connects → mock accepts one harness connection → on the next engine
//   request, sends the device-side fd over the harness connection via SCM_RIGHTS.
//   Repeat: for each new engine request, the harness must reconnect to receive
//   the next fd.

use lam_hidraw_helper::{mock, server};
use std::os::unix::io::{AsRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use tokio::net::UnixListener;

fn helper_socket_path() -> PathBuf {
    std::env::var("LAM_HELPER_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let dir = std::env::var("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR is not set");
            PathBuf::from(dir).join("lam-hidraw-helper-mock.sock")
        })
}

fn harness_socket_path(helper: &Path) -> PathBuf {
    std::env::var("LAM_HARNESS_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let mut s = helper.as_os_str().to_owned();
            s.push(".harness");
            PathBuf::from(s)
        })
}

#[tokio::main]
async fn main() {
    let helper_path = helper_socket_path();
    let harness_path = harness_socket_path(&helper_path);

    let helper_listener =
        server::bind_socket(&helper_path).expect("cannot bind helper mock socket");
    let harness_listener = server::bind_socket(&harness_path).expect("cannot bind harness socket");

    eprintln!(
        "lam-hidraw-helper-mock listening on {}",
        helper_path.display()
    );
    eprintln!("harness socket: {}", harness_path.display());

    let (tx, rx) = tokio::sync::mpsc::channel::<OwnedFd>(8);

    tokio::spawn(mock::serve_mock(helper_listener, tx));
    tokio::spawn(serve_harness(harness_listener, rx));

    tokio::signal::ctrl_c().await.ok();
}

// Pairs each device-side fd from the mock with one harness connection.
// Harness must reconnect for each new device fd it wants to receive.
async fn serve_harness(listener: UnixListener, mut rx: tokio::sync::mpsc::Receiver<OwnedFd>) {
    loop {
        // Accept harness connection first; harness drives the pacing.
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        // Wait for the next device-side fd from the mock.
        let Some(fd) = rx.recv().await else { return };
        if let Err(e) = server::send_fd(&stream, fd.as_raw_fd()) {
            eprintln!("harness: send_fd failed: {e}");
        }
    }
}
