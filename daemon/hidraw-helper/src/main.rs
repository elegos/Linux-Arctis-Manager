use lam_hidraw_helper::server;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::info;

fn socket_path() -> PathBuf {
    let dir = std::env::var("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR is not set");
    PathBuf::from(dir).join("lam-hidraw-helper.sock")
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let path = socket_path();
    let listener = server::bind_socket(&path)
        .unwrap_or_else(|e| panic!("cannot bind {}: {e}", path.display()));

    info!(
        "lam-hidraw-helper {} listening on {}",
        env!("LAM_VERSION"),
        path.display()
    );

    server::serve(listener, Arc::new(server::HelperConfig::system())).await;
}
