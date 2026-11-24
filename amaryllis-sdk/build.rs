fn main() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set");
    println!("cargo:rustc-link-arg=-T{}/linker.ld", dir);
    println!("cargo:rerun-if-changed=linker.ld");
}
