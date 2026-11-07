fn main() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    println!("cargo:rustc-env=BUILT_AT={now}");
    println!("cargo:rerun-if-changed=build.rs");
}
