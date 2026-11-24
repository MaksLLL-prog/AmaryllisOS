use std::env;
use std::fs;
use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=assets/ui.ttf");
    println!("cargo:rerun-if-changed=assets/mono.ttf");

    let out_dir = env::var("OUT_DIR").expect("OUT_DIR missing");
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR missing");

    let ui_exists = Path::new(&manifest_dir).join("assets/ui.ttf").is_file();
    let mono_exists = Path::new(&manifest_dir).join("assets/mono.ttf").is_file();

    let mut out = String::new();
    if ui_exists {
        out.push_str(
            "pub static UI_FONT_BYTES: &[u8] = include_bytes!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/assets/ui.ttf\"));\n",
        );
    } else {
        out.push_str("pub static UI_FONT_BYTES: &[u8] = &[];\n");
    }

    if mono_exists {
        out.push_str(
            "pub static MONO_FONT_BYTES: &[u8] = include_bytes!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/assets/mono.ttf\"));\n",
        );
    } else {
        out.push_str("pub static MONO_FONT_BYTES: &[u8] = &[];\n");
    }

    let dest = Path::new(&out_dir).join("embedded_fonts.rs");
    fs::write(dest, out).expect("failed to write embedded_fonts.rs");
}
