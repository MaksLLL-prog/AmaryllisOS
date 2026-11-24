
use super::{get_device, PciBar, PciBarKind, MAX_PCI_DEVICES};

pub const MAX_PCI_RESOURCES: usize = MAX_PCI_DEVICES * 6;

#[derive(Clone, Copy)]
pub struct PciResource {
    pub bus: u8,
    pub device: u8,
    pub function: u8,

    pub vendor_id: u16,
    pub device_id: u16,

    pub bar: PciBar,
}

impl PciResource {
    pub const EMPTY: Self = Self {
        bus: 0,
        device: 0,
        function: 0,
        vendor_id: 0,
        device_id: 0,
        bar: PciBar::EMPTY,
    };

    pub fn is_memory(&self) -> bool {
        matches!(
            self.bar.kind,
            PciBarKind::Memory32
                | PciBarKind::MemoryBelow1M
                | PciBarKind::Memory64
        )
    }

    pub fn is_io(&self) -> bool {
        self.bar.kind == PciBarKind::Io
    }

    pub fn end_address(&self) -> Option<u64> {
        if !self.bar.size_known || self.bar.size == 0 {
            return None;
        }

        self.bar
            .base
            .checked_add(self.bar.size - 1)
    }
}

#[derive(Clone, Copy)]
pub struct ResourceStats {
    pub devices_scanned: usize,
    pub resources_registered: usize,
    pub unknown_sizes: usize,
    pub overflow: usize,
}

impl ResourceStats {
    pub const EMPTY: Self = Self {
        devices_scanned: 0,
        resources_registered: 0,
        unknown_sizes: 0,
        overflow: 0,
    };
}

struct ResourceRegistry {
    resources: [PciResource; MAX_PCI_RESOURCES],
    count: usize,
    stats: ResourceStats,
}

impl ResourceRegistry {
    const fn new() -> Self {
        Self {
            resources: [PciResource::EMPTY; MAX_PCI_RESOURCES],
            count: 0,
            stats: ResourceStats::EMPTY,
        }
    }
}

static mut RESOURCE_REGISTRY: ResourceRegistry =
    ResourceRegistry::new();

// ============================================================
// REGISTRY BUILD
// ============================================================

/// Rebuilds the resource registry from the scanned PCI inventory.
///
/// Call this after PCI scanning has completed.
pub fn rebuild() -> ResourceStats {
    let registry_ptr =
        core::ptr::addr_of_mut!(RESOURCE_REGISTRY);

    unsafe {
        let registry = &mut *registry_ptr;

        registry.count = 0;
        registry.stats = ResourceStats::EMPTY;
    }

    let mut stats = ResourceStats::EMPTY;

    for device_index in 0..MAX_PCI_DEVICES {
        let device = match get_device(device_index) {
            Some(device) => device,
            None => break,
        };

        stats.devices_scanned += 1;

        let bar_limit = core::cmp::min(
            device.bar_count,
            device.bars.len(),
        );

        for bar_index in 0..bar_limit {
            let bar = device.bars[bar_index];

            if !bar.valid {
                continue;
            }

            let resource = PciResource {
                bus: device.bus,
                device: device.device,
                function: device.function,
                vendor_id: device.vendor_id,
                device_id: device.device_id,
                bar,
            };

            let registry_ptr =
                core::ptr::addr_of_mut!(RESOURCE_REGISTRY);

            unsafe {
                let registry = &mut *registry_ptr;

                if registry.count >= MAX_PCI_RESOURCES {
                    stats.overflow += 1;
                    continue;
                }

                registry.resources[registry.count] = resource;
                registry.count += 1;
            }

            stats.resources_registered += 1;

            if !bar.size_known {
                stats.unknown_sizes += 1;
            }
        }
    }

    let registry_ptr =
        core::ptr::addr_of_mut!(RESOURCE_REGISTRY);

    unsafe {
        let registry = &mut *registry_ptr;
        registry.stats = stats;
    }

    stats
}

// ============================================================
// REGISTRY ACCESS
// ============================================================

pub fn count() -> usize {
    let registry_ptr =
        core::ptr::addr_of!(RESOURCE_REGISTRY);

    unsafe {
        (*registry_ptr).count
    }
}

pub fn get(index: usize) -> Option<PciResource> {
    let registry_ptr =
        core::ptr::addr_of!(RESOURCE_REGISTRY);

    unsafe {
        let registry = &*registry_ptr;

        if index < registry.count {
            Some(registry.resources[index])
        } else {
            None
        }
    }
}

pub fn statistics() -> ResourceStats {
    let registry_ptr =
        core::ptr::addr_of!(RESOURCE_REGISTRY);

    unsafe {
        (*registry_ptr).stats
    }
}

// ============================================================
// RESOURCE SEARCH
// ============================================================

pub fn find_bar(
    bus: u8,
    device: u8,
    function: u8,
    bar_index: u8,
) -> Option<PciResource> {
    let total = count();

    for index in 0..total {
        let resource = get(index)?;

        if resource.bus == bus
            && resource.device == device
            && resource.function == function
            && resource.bar.index == bar_index
        {
            return Some(resource);
        }
    }

    None
}

pub fn find_by_device(
    bus: u8,
    device: u8,
    function: u8,
    output: &mut [PciResource],
) -> usize {
    let total = count();
    let mut found = 0;

    for index in 0..total {
        if found >= output.len() {
            break;
        }

        let resource = match get(index) {
            Some(resource) => resource,
            None => break,
        };

        if resource.bus == bus
            && resource.device == device
            && resource.function == function
        {
            output[found] = resource;
            found += 1;
        }
    }

    found
}
