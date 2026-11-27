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
    if std::path::Path::new("disk_nvme.img").exists() {
        args.extend([
            "-drive".to_string(),
            "file=disk_nvme.img,if=none,id=nvm0,format=raw".to_string(),
            "-device".to_string(),
            "nvme,serial=AMARYLLISNVME01,drive=nvm0".to_string(),
        ]);
    }
    args.extend([
        "-device".to_string(),
        "qemu-xhci,id=xhci0".to_string(),
        "-device".to_string(),
        "usb-kbd,bus=xhci0.0".to_string(),
        "-device".to_string(),
        "usb-mouse,bus=xhci0.0".to_string(),
        "-netdev".to_string(),
        "user,id=net_re".to_string(),
        "-device".to_string(),
        "rtl8139,netdev=net_re".to_string(),
        "-device".to_string(),
        "intel-hda".to_string(),
        "-audiodev".to_string(), "pa,id=snd_hda".to_string(), "-device".to_string(), "hda-duplex,audiodev=snd_hda".to_string(),
    ]);
    if std::path::Path::new("usb_flash.img").exists() {
        args.extend([
            "-drive".to_string(),
            "file=usb_flash.img,if=none,id=usb0,format=raw".to_string(),
            "-device".to_string(),
            "usb-storage,bus=xhci0.0,drive=usb0".to_string(),
        ]);
    }
    if std::path::Path::new("sdcard.img").exists() {
        args.extend([
            "-drive".to_string(),
            "file=sdcard.img,if=none,id=sd0,format=raw".to_string(),
            "-device".to_string(),
            "sdhci-pci".to_string(),
            "-device".to_string(),
            "sd-card,drive=sd0".to_string(),
        ]);
    }
    let status = Command::new("qemu-system-x86_64").args(&args).status().expect("Failed to start QEMU");
    println!("QEMU exited with: {status}");
}
