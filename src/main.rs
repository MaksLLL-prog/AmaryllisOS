use std::process::Command;

fn main() {
    println!("Starting AmaryllisOS...");
    let mut args = vec![
        "-enable-kvm".to_string(), "-cpu".to_string(), "host".to_string(),
        "-smp".to_string(), "4".to_string(),
        "-drive".to_string(), format!("format=raw,file={}", env!("BIOS_PATH")),
        "-m".to_string(), "1G".to_string(),
        "-serial".to_string(), "stdio".to_string(),
        "-netdev".to_string(), "user,id=net0".to_string(),
        "-device".to_string(), "e1000,netdev=net0".to_string(),
        "-audiodev".to_string(), "pa,id=snd0".to_string(),
        "-device".to_string(), "AC97,audiodev=snd0".to_string(),
    ];
    if std::path::Path::new("disk_virtio.img").exists() {
        args.extend(["-drive".to_string(), "file=disk_virtio.img,if=virtio,format=raw".to_string()]);
    }
    let status = Command::new("qemu-system-x86_64").args(&args).status().expect("Failed to start QEMU");
    println!("QEMU exited with: {status}");
}
