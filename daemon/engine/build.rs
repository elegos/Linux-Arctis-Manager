fn main() {
    println!("cargo:rerun-if-env-changed=LAM_DATADIR");
}
