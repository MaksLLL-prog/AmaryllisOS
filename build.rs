use std::path::PathBuf;

fn main() {
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR missing"));
    let kernel = PathBuf::from(
        std::env::var_os("CARGO_BIN_FILE_KERNEL_kernel")
            .or_else(|| std::env::var_os("CARGO_BIN_FILE_KERNEL"))
            .expect("CARGO_BIN_FILE_KERNEL_kernel missing"),
    );

    let mut boot_config = bootloader::BootConfig::default();
    boot_config.frame_buffer.minimum_framebuffer_width = Some(1280);
    boot_config.frame_buffer.minimum_framebuffer_height = Some(720);

    let bios_path = out_dir.join("bios.img");
    bootloader::BiosBoot::new(&kernel)
        .set_boot_config(&boot_config)
        .create_disk_image(&bios_path)
        .expect("Failed to create BIOS disk image");

    let uefi_path = out_dir.join("uefi.img");
    bootloader::UefiBoot::new(&kernel)
        .set_boot_config(&boot_config)
        .create_disk_image(&uefi_path)
        .expect("Failed to create UEFI disk image");

    let release_dir = PathBuf::from("target/release");
    let _ = std::fs::create_dir_all(&release_dir);
    let _ = std::fs::copy(&bios_path, release_dir.join("amaryllis_bios.img"));
    let _ = std::fs::copy(&uefi_path, release_dir.join("amaryllis_uefi.img"));

    println!("cargo:rustc-env=BIOS_PATH={}", bios_path.display());
    println!("cargo:rustc-env=UEFI_PATH={}", uefi_path.display());
}
