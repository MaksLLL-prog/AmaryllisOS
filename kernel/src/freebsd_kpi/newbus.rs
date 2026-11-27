use alloc::vec::Vec;

use crate::memory::vmm::PhysicalMemoryMapping;
use crate::memory::PhysicalFrameAllocator;
use crate::pci::PciDevice;
use crate::task::preempt::PreemptMutex;

use super::KpiError;

pub const BUS_PROBE_SPECIFIC: i32 = 0;
pub const BUS_PROBE_DEFAULT: i32 = -20;
pub const ENXIO: i32 = 6;

pub type DeviceId = usize;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum NewbusState {
    Detected,
    Probed,
    Attached,
    Detached,
    Failed,
}

impl NewbusState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Detected => "DETECTED",
            Self::Probed => "PROBED",
            Self::Attached => "ATTACHED",
            Self::Detached => "DETACHED",
            Self::Failed => "FAILED",
        }
    }
}

#[derive(Clone, Copy)]
pub struct DeviceT {
    pub id: DeviceId,
    pub unit: u16,
    pub name: &'static str,
    pub desc: [u8; 48],
    pub desc_len: usize,
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class_code: u8,
    pub subclass: u8,
    pub progif: u8,
    pub driver_name: &'static str,
    pub state: NewbusState,
    pub bar0_phys: u64,
    pub bar0_virt: u64,
    pub irq_line: u8,
    pub irq_gsi: Option<u32>,
    pub irq_vector: Option<u8>,
}

impl DeviceT {
    pub const EMPTY: Self = Self {
        id: 0,
        unit: 0,
        name: "",
        desc: [0; 48],
        desc_len: 0,
        bus: 0,
        device: 0,
        function: 0,
        vendor_id: 0,
        device_id: 0,
        class_code: 0,
        subclass: 0,
        progif: 0,
        driver_name: "",
        state: NewbusState::Detected,
        bar0_phys: 0,
        bar0_virt: 0,
        irq_line: 0,
        irq_gsi: None,
        irq_vector: None,
    };

    pub fn set_desc(&mut self, text: &str) {
        let bytes = text.as_bytes();
        let n = bytes.len().min(self.desc.len());
        self.desc[..n].copy_from_slice(&bytes[..n]);
        self.desc_len = n;
    }

    pub fn desc_str(&self) -> &str {
        core::str::from_utf8(&self.desc[..self.desc_len]).unwrap_or("")
    }
}

#[derive(Clone, Copy)]
pub struct DriverT {
    pub name: &'static str,
    pub probe: fn(&DeviceT) -> i32,
    pub attach: fn(DeviceId, Option<&PhysicalMemoryMapping>, &mut PhysicalFrameAllocator<'_>) -> i32,
    pub detach: fn(DeviceId) -> i32,
}

static NEWBUS_DEVICES: PreemptMutex<Vec<DeviceT>> = PreemptMutex::new(Vec::new());

pub fn device_add_pci_child(pci_dev: &PciDevice, name: &'static str) -> DeviceId {
    let mut devs = NEWBUS_DEVICES.lock();
    let unit = devs.iter().filter(|d| d.name == name).count() as u16;
    let id = devs.len();
    let mut dev = DeviceT::EMPTY;
    dev.id = id;
    dev.unit = unit;
    dev.name = name;
    dev.bus = pci_dev.bus;
    dev.device = pci_dev.device;
    dev.function = pci_dev.function;
    dev.vendor_id = pci_dev.vendor_id;
    dev.device_id = pci_dev.device_id;
    dev.class_code = pci_dev.class_code;
    dev.subclass = pci_dev.subclass;
    dev.progif = pci_dev.programming_interface;
    dev.bar0_phys = pci_dev
        .bars
        .iter()
        .find(|b| b.valid && b.base != 0)
        .map(|b| b.base)
        .unwrap_or(0);
    devs.push(dev);
    id
}

pub fn with_device<F, R>(dev_id: DeviceId, f: F) -> Option<R>
where
    F: FnOnce(&DeviceT) -> R,
{
    let devs = NEWBUS_DEVICES.lock();
    devs.get(dev_id).map(f)
}

pub fn with_device_mut<F, R>(dev_id: DeviceId, f: F) -> Option<R>
where
    F: FnOnce(&mut DeviceT) -> R,
{
    let mut devs = NEWBUS_DEVICES.lock();
    devs.get_mut(dev_id).map(f)
}

pub fn device_probe_and_attach(
    dev_id: DeviceId,
    driver: &DriverT,
    mapping: Option<&PhysicalMemoryMapping>,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> Result<(), KpiError> {
    let probe_rc = {
        let mut devs = NEWBUS_DEVICES.lock();
        let dev = devs.get_mut(dev_id).ok_or(KpiError::NoDev)?;
        let rc = (driver.probe)(dev);
        if rc <= 0 {
            dev.driver_name = driver.name;
            dev.state = NewbusState::Probed;
        }
        rc
    };
    if probe_rc > 0 {
        return Err(KpiError::NoDev);
    }

    let attach_rc = (driver.attach)(dev_id, mapping, allocator);
    let mut devs = NEWBUS_DEVICES.lock();
    let dev = devs.get_mut(dev_id).ok_or(KpiError::NoDev)?;
    if attach_rc == 0 {
        dev.state = NewbusState::Attached;
        Ok(())
    } else {
        dev.state = NewbusState::Failed;
        Err(KpiError::Io)
    }
}

pub fn device_detach(dev_id: DeviceId, driver: &DriverT) -> Result<(), KpiError> {
    let rc = (driver.detach)(dev_id);
    if rc == 0 {
        if let Some(dev) = NEWBUS_DEVICES.lock().get_mut(dev_id) {
            dev.state = NewbusState::Detached;
        }
        Ok(())
    } else {
        Err(KpiError::Io)
    }
}

pub fn snapshot_devices() -> Vec<DeviceT> {
    NEWBUS_DEVICES.lock().clone()
}
