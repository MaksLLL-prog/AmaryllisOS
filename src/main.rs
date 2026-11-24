use std::process::Command;

fn main() {
    println!("Starting AmaryllisOS...");

    let status = Command::new(r"C:\Program Files\qemu\qemu-system-x86_64.exe")
        .args([
            "-drive",
            &format!("format=raw,file={}", env!("BIOS_PATH")),
            "-m",
            "512M",
        ])
        .status()
        .expect("Failed to start QEMU");

    println!("QEMU exited with: {status}");
}
