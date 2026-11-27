use core::fmt;

use crate::task::preempt::PreemptMutex;

pub const MAX_DRIVERS: usize = 48;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DeviceClass {
    Timer,
    Keyboard,
    Mouse,
    Framebuffer,
    Storage,
    Network,
    Audio,
    Controller,
    Usb,
    CardReader,
    Power,
}

impl DeviceClass {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Timer => "TIMER",
            Self::Keyboard => "KEYBOARD",
            Self::Mouse => "MOUSE",
            Self::Framebuffer => "DISPLAY",
            Self::Storage => "STORAGE",
            Self::Network => "NETWORK",
            Self::Audio => "AUDIO",
            Self::Controller => "CTRL",
            Self::Usb => "USB",
            Self::CardReader => "CARD",
            Self::Power => "POWER",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DeviceBus {
    System,
    Ps2,
    Framebuffer,
    Pci {
        bus: u8,
        device: u8,
        function: u8,
        vendor_id: u16,
        device_id: u16,
    },
}

impl DeviceBus {
    pub fn format_short<'a>(&self, buf: &'a mut [u8; 32]) -> &'a str {
        struct BufWriter<'a> {
            buf: &'a mut [u8],
            pos: usize,
        }
        impl fmt::Write for BufWriter<'_> {
            fn write_str(&mut self, s: &str) -> fmt::Result {
                let bytes = s.as_bytes();
                let avail = self.buf.len().saturating_sub(self.pos);
                let n = bytes.len().min(avail);
                self.buf[self.pos..self.pos + n].copy_from_slice(&bytes[..n]);
                self.pos += n;
                Ok(())
            }
        }
        let mut w = BufWriter { buf, pos: 0 };
        match *self {
            Self::System => {
                let _ = fmt::Write::write_str(&mut w, "SYSTEM");
            }
            Self::Ps2 => {
                let _ = fmt::Write::write_str(&mut w, "PS/2");
            }
            Self::Framebuffer => {
                let _ = fmt::Write::write_str(&mut w, "LINEAR-FB");
            }
            Self::Pci {
                bus,
                device,
                function,
                vendor_id,
                device_id,
            } => {
                let _ = fmt::write(
                    &mut w,
                    format_args!(
                        "PCI {:02}:{:02}.{} {:04X}:{:04X}",
                        bus, device, function, vendor_id, device_id
                    ),
                );
            }
        }
        let len = w.pos;
        core::str::from_utf8(&buf[..len]).unwrap_or("?")
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DriverStatus {
    Registered,
    Active,
    Failed,
    NotPresent,
}

impl DriverStatus {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Registered => "REGISTERED",
            Self::Active => "ACTIVE",
            Self::Failed => "FAILED",
            Self::NotPresent => "NOT PRESENT",
        }
    }
}

pub trait KernelDriver {
    fn driver_name(&self) -> &'static str;
    fn device_class(&self) -> DeviceClass;
    fn status(&self) -> DriverStatus;
}

pub trait NetworkDevice: KernelDriver {
    fn mac_address(&self) -> [u8; 6];
    fn link_up(&self) -> bool;
    fn send_frame(&mut self, frame: &[u8]) -> Result<(), &'static str>;
    fn poll_frame(&mut self, out: &mut [u8]) -> Option<usize>;
}

pub trait AudioDevice: KernelDriver {
    fn sample_rate(&self) -> u32;
    fn set_sample_rate(&mut self, rate_hz: u32) -> u32;
    fn set_volume(&mut self, percent: u8);
    fn stop(&mut self);
}

#[derive(Clone, Copy)]
pub struct DeviceEntry {
    pub id: usize,
    pub name: &'static str,
    pub driver_name: &'static str,
    pub class: DeviceClass,
    pub bus: DeviceBus,
    pub status: DriverStatus,
    pub irq: Option<u8>,
    pub io_base: u64,
    pub mmio_base: u64,
    pub details: [u8; 64],
    pub details_len: usize,
    pub valid: bool,
}

impl DeviceEntry {
    pub const EMPTY: Self = Self {
        id: 0,
        name: "",
        driver_name: "",
        class: DeviceClass::Controller,
        bus: DeviceBus::System,
        status: DriverStatus::NotPresent,
        irq: None,
        io_base: 0,
        mmio_base: 0,
        details: [0; 64],
        details_len: 0,
        valid: false,
    };

    pub fn details_str(&self) -> &str {
        core::str::from_utf8(&self.details[..self.details_len]).unwrap_or("")
    }

    pub fn set_details(&mut self, text: &str) {
        let bytes = text.as_bytes();
        let len = bytes.len().min(self.details.len());
        self.details[..len].copy_from_slice(&bytes[..len]);
        self.details_len = len;
    }
}

pub struct DriverRegistry {
    entries: [DeviceEntry; MAX_DRIVERS],
    count: usize,
    overflow: usize,
}

impl DriverRegistry {
    pub const fn new() -> Self {
        Self {
            entries: [DeviceEntry::EMPTY; MAX_DRIVERS],
            count: 0,
            overflow: 0,
        }
    }

    pub fn clear(&mut self) {
        for entry in &mut self.entries {
            *entry = DeviceEntry::EMPTY;
        }
        self.count = 0;
        self.overflow = 0;
    }

    pub fn register(&mut self, mut entry: DeviceEntry) -> Option<usize> {
        if self.count >= MAX_DRIVERS {
            self.overflow += 1;
            return None;
        }
        let id = self.count;
        entry.id = id;
        entry.valid = true;
        self.entries[id] = entry;
        self.count += 1;
        Some(id)
    }

    pub fn update_status(&mut self, name: &'static str, status: DriverStatus, details: &str) {
        for index in 0..self.count {
            if self.entries[index].valid && self.entries[index].name == name {
                self.entries[index].status = status;
                self.entries[index].set_details(details);
                return;
            }
        }
    }

    pub fn count(&self) -> usize {
        self.count
    }

    pub fn overflow(&self) -> usize {
        self.overflow
    }

    pub fn get(&self, index: usize) -> Option<DeviceEntry> {
        if index < self.count && self.entries[index].valid {
            Some(self.entries[index])
        } else {
            None
        }
    }

    pub fn find_by_class(&self, class: DeviceClass) -> Option<DeviceEntry> {
        for index in 0..self.count {
            let entry = self.entries[index];
            if entry.valid && entry.class == class {
                return Some(entry);
            }
        }
        None
    }
}

pub static DRIVER_REGISTRY: PreemptMutex<DriverRegistry> = PreemptMutex::new(DriverRegistry::new());

pub fn register_device(
    name: &'static str,
    driver_name: &'static str,
    class: DeviceClass,
    bus: DeviceBus,
    status: DriverStatus,
    irq: Option<u8>,
    io_base: u64,
    mmio_base: u64,
    details: &str,
) -> Option<usize> {
    let mut entry = DeviceEntry::EMPTY;
    entry.name = name;
    entry.driver_name = driver_name;
    entry.class = class;
    entry.bus = bus;
    entry.status = status;
    entry.irq = irq;
    entry.io_base = io_base;
    entry.mmio_base = mmio_base;
    entry.set_details(details);
    DRIVER_REGISTRY.lock().register(entry)
}

pub fn count() -> usize {
    DRIVER_REGISTRY.lock().count()
}

pub fn get(index: usize) -> Option<DeviceEntry> {
    DRIVER_REGISTRY.lock().get(index)
}
