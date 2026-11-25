use std::path::PathBuf;

fn main() {
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let kernel = PathBuf::from(
        std::env::var_os("CARGO_BIN_FILE_KERNEL_kernel").unwrap()
    );

    let bios_path = out_dir.join("bios.img");

    let mut boot_config = bootloader::BootConfig::default();
    boot_config.frame_buffer.minimum_framebuffer_width = Some(1600);
    boot_config.frame_buffer.minimum_framebuffer_height = Some(900);

    let mut bios_boot = bootloader::BiosBoot::new(&kernel);
    bios_boot.set_boot_config(&boot_config);
    bios_boot
        .create_disk_image(&bios_path)
        .expect("Failed to create BIOS disk image");

    println!("cargo:rustc-env=BIOS_PATH={}", bios_path.display());
}