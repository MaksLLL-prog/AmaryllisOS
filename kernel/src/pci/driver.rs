
use crate::console::Console;
use crate::graphics::Color;

use super::{
    PciBarKind,
    PciDevice,
    PciInventory,
    MAX_PCI_DEVICES,
};

const WHITE: Color = (235, 235, 240);
const GREEN: Color = (40, 180, 120);
const GRAY: Color = (140, 140, 150);
const YELLOW: Color = (220, 190, 90);
const RED: Color = (220, 90, 90);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DriverLifecycle {
    Registered,
    Matched,
    Initializing,
    Initialized,
    Failed,
}

impl DriverLifecycle {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Registered => "REGISTERED",
            Self::Matched => "MATCHED",
            Self::Initializing => "INITIALIZING",
            Self::Initialized => "INITIALIZED",
            Self::Failed => "FAILED",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BindingState {
    Matched,
    ProbeRejected,
    NoDriver,
}

impl BindingState {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Matched => "MATCHED",
            Self::ProbeRejected => "PROBE REJECTED",
            Self::NoDriver => "NO DRIVER",
        }
    }
}

// ============================================================
// DEVICE MATCHER
// ============================================================

#[derive(Clone, Copy)]
pub struct DeviceMatcher {
    pub vendor_id: Option<u16>,
    pub device_id: Option<u16>,
    pub class_code: Option<u8>,
    pub subclass: Option<u8>,
    pub programming_interface: Option<u8>,
}

impl DeviceMatcher {
    pub const fn matches(&self, device: &PciDevice) -> bool {
        option_matches_u16(self.vendor_id, device.vendor_id)
            && option_matches_u16(self.device_id, device.device_id)
            && option_matches_u8(self.class_code, device.class_code)
            && option_matches_u8(self.subclass, device.subclass)
            && option_matches_u8(
                self.programming_interface,
                device.programming_interface,
            )
    }

    pub const fn specificity(&self) -> u8 {
        let mut score = 0;

        if self.vendor_id.is_some() {
            score += 1;
        }
        if self.device_id.is_some() {
            score += 1;
        }
        if self.class_code.is_some() {
            score += 1;
        }
        if self.subclass.is_some() {
            score += 1;
        }
        if self.programming_interface.is_some() {
            score += 1;
        }

        score
    }
}

const fn option_matches_u16(
    expected: Option<u16>,
    actual: u16,
) -> bool {
    match expected {
        Some(value) => value == actual,
        None => true,
    }
}

const fn option_matches_u8(
    expected: Option<u8>,
    actual: u8,
) -> bool {
    match expected {
        Some(value) => value == actual,
        None => true,
    }
}

// ============================================================
// DRIVER PROBE
// ============================================================

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ProbeResult {
    Compatible,
    Rejected,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DriverInitResult {
    Success,
    NotImplemented,
    HardwareError,
}

pub struct DriverDescriptor {
    pub name: &'static str,
    pub matcher: DeviceMatcher,
    pub priority: u8,
    pub probe: fn(&PciDevice) -> ProbeResult,
    pub initialize: fn(&PciDevice) -> DriverInitResult,
}

// ============================================================
// AHCI
// ============================================================

fn probe_ahci(device: &PciDevice) -> ProbeResult {
    if device.class_code != 0x01
        || device.subclass != 0x06
        || device.programming_interface != 0x01
    {
        return ProbeResult::Rejected;
    }

    for index in 0..device.bar_count {
        let bar = device.bars[index];

        if bar.index == 5
            && bar.valid
            && matches!(
                bar.kind,
                PciBarKind::Memory32 | PciBarKind::Memory64
            )
            && bar.base != 0
        {
            return ProbeResult::Compatible;
        }
    }

    ProbeResult::Rejected
}

// ============================================================
// NVME
// ============================================================

fn probe_nvme(device: &PciDevice) -> ProbeResult {
    if device.class_code != 0x01
        || device.subclass != 0x08
        || device.programming_interface != 0x02
    {
        return ProbeResult::Rejected;
    }

    for index in 0..device.bar_count {
        let bar = device.bars[index];

        if bar.index == 0
            && bar.valid
            && matches!(
                bar.kind,
                PciBarKind::Memory32 | PciBarKind::Memory64
            )
            && bar.base != 0
        {
            return ProbeResult::Compatible;
        }
    }

    ProbeResult::Rejected
}

// ============================================================
// IDE
// ============================================================

fn probe_ide(device: &PciDevice) -> ProbeResult {
    if device.class_code == 0x01
        && device.subclass == 0x01
    {
        ProbeResult::Compatible
    } else {
        ProbeResult::Rejected
    }
}

fn probe_e1000(device: &PciDevice) -> ProbeResult {
    if device.vendor_id == 0x8086
        && matches!(
            device.device_id,
            0x100E
                | 0x100F
                | 0x10D3
                | 0x1004
                | 0x1019
                | 0x10EA
                | 0x10EF
                | 0x1502
                | 0x1503
                | 0x153A
                | 0x153B
                | 0x15B7
                | 0x15B8
                | 0x15BC
                | 0x15D8
                | 0x15F3
        )
    {
        ProbeResult::Compatible
    } else {
        ProbeResult::Rejected
    }
}

fn probe_re_nic(device: &PciDevice) -> ProbeResult {
    if device.vendor_id == 0x10EC
        && device.class_code == 0x02
        && device.subclass == 0x00
        && matches!(
            device.device_id,
            0x8139 | 0x8161 | 0x8167 | 0x8168 | 0x8169 | 0x8136 | 0x8125 | 0x2600 | 0x3000
        )
    {
        ProbeResult::Compatible
    } else {
        ProbeResult::Rejected
    }
}

fn probe_ac97(device: &PciDevice) -> ProbeResult {
    if device.vendor_id == 0x8086
        && (device.device_id == 0x2415
            || (device.class_code == 0x04 && device.subclass == 0x01))
    {
        ProbeResult::Compatible
    } else {
        ProbeResult::Rejected
    }
}

fn probe_hda(device: &PciDevice) -> ProbeResult {
    if device.class_code == 0x04 && device.subclass == 0x03 {
        ProbeResult::Compatible
    } else {
        ProbeResult::Rejected
    }
}

fn probe_xhci(device: &PciDevice) -> ProbeResult {
    if device.class_code == 0x0C
        && device.subclass == 0x03
        && device.programming_interface == 0x30
    {
        ProbeResult::Compatible
    } else {
        ProbeResult::Rejected
    }
}

fn probe_sdhci(device: &PciDevice) -> ProbeResult {
    if device.class_code == 0x08 && device.subclass == 0x05 {
        ProbeResult::Compatible
    } else {
        ProbeResult::Rejected
    }
}

fn probe_display(device: &PciDevice) -> ProbeResult {
    if device.class_code == 0x03 {
        ProbeResult::Compatible
    } else {
        ProbeResult::Rejected
    }
}

// ============================================================
// DRIVER INITIALIZATION
// ============================================================

fn initialize_ahci(_device: &PciDevice) -> DriverInitResult {
    DriverInitResult::Success
}

fn initialize_ide(_device: &PciDevice) -> DriverInitResult {
    DriverInitResult::Success
}

fn initialize_nvme(_device: &PciDevice) -> DriverInitResult {
    DriverInitResult::Success
}

fn initialize_e1000(_device: &PciDevice) -> DriverInitResult {
    DriverInitResult::Success
}

fn initialize_re_nic(_device: &PciDevice) -> DriverInitResult {
    DriverInitResult::Success
}

fn initialize_ac97(_device: &PciDevice) -> DriverInitResult {
    DriverInitResult::Success
}

fn initialize_hda(_device: &PciDevice) -> DriverInitResult {
    DriverInitResult::Success
}

fn initialize_xhci(_device: &PciDevice) -> DriverInitResult {
    DriverInitResult::Success
}

fn initialize_sdhci(_device: &PciDevice) -> DriverInitResult {
    DriverInitResult::Success
}

fn initialize_display(_device: &PciDevice) -> DriverInitResult {
    DriverInitResult::Success
}

// ============================================================
// DRIVER REGISTRY
// ============================================================

static DRIVERS: &[DriverDescriptor] = &[
    DriverDescriptor {
        name: "AHCI SATA",
        matcher: DeviceMatcher {
            vendor_id: None,
            device_id: None,
            class_code: Some(0x01),
            subclass: Some(0x06),
            programming_interface: Some(0x01),
        },
        priority: 100,
        initialize: initialize_ahci,
        probe: probe_ahci,
    },
    DriverDescriptor {
        name: "IDE Controller",
        matcher: DeviceMatcher {
            vendor_id: None,
            device_id: None,
            class_code: Some(0x01),
            subclass: Some(0x01),
            programming_interface: None,
        },
        priority: 90,
        probe: probe_ide,
        initialize: initialize_ide,
    },
    DriverDescriptor {
        name: "NVMe",
        matcher: DeviceMatcher {
            vendor_id: None,
            device_id: None,
            class_code: Some(0x01),
            subclass: Some(0x08),
            programming_interface: Some(0x02),
        },
        priority: 100,
        probe: probe_nvme,
        initialize: initialize_nvme,
    },
    DriverDescriptor {
        name: "Intel e1000 NIC",
        matcher: DeviceMatcher {
            vendor_id: Some(0x8086),
            device_id: None,
            class_code: Some(0x02),
            subclass: Some(0x00),
            programming_interface: None,
        },
        priority: 100,
        probe: probe_e1000,
        initialize: initialize_e1000,
    },
    DriverDescriptor {
        name: "Realtek Ethernet (re/rl)",
        matcher: DeviceMatcher {
            vendor_id: Some(0x10EC),
            device_id: None,
            class_code: Some(0x02),
            subclass: Some(0x00),
            programming_interface: None,
        },
        priority: 100,
        probe: probe_re_nic,
        initialize: initialize_re_nic,
    },
    DriverDescriptor {
        name: "Intel AC'97 Audio",
        matcher: DeviceMatcher {
            vendor_id: Some(0x8086),
            device_id: Some(0x2415),
            class_code: Some(0x04),
            subclass: Some(0x01),
            programming_interface: None,
        },
        priority: 100,
        probe: probe_ac97,
        initialize: initialize_ac97,
    },
    DriverDescriptor {
        name: "Intel HDA Audio",
        matcher: DeviceMatcher {
            vendor_id: None,
            device_id: None,
            class_code: Some(0x04),
            subclass: Some(0x03),
            programming_interface: None,
        },
        priority: 100,
        probe: probe_hda,
        initialize: initialize_hda,
    },
    DriverDescriptor {
        name: "USB xHCI Host Controller",
        matcher: DeviceMatcher {
            vendor_id: None,
            device_id: None,
            class_code: Some(0x0C),
            subclass: Some(0x03),
            programming_interface: Some(0x30),
        },
        priority: 100,
        probe: probe_xhci,
        initialize: initialize_xhci,
    },
    DriverDescriptor {
        name: "SDHCI Card Reader",
        matcher: DeviceMatcher {
            vendor_id: None,
            device_id: None,
            class_code: Some(0x08),
            subclass: Some(0x05),
            programming_interface: None,
        },
        priority: 100,
        probe: probe_sdhci,
        initialize: initialize_sdhci,
    },
    DriverDescriptor {
        name: "UEFI GOP / PCI Display",
        matcher: DeviceMatcher {
            vendor_id: None,
            device_id: None,
            class_code: Some(0x03),
            subclass: None,
            programming_interface: None,
        },
        priority: 80,
        probe: probe_display,
        initialize: initialize_display,
    },
];

// ============================================================
// BINDING TYPES
// ============================================================

#[derive(Clone, Copy)]
pub struct DeviceBinding {
    pub bus: u8,
    pub device: u8,
    pub function: u8,

    pub vendor_id: u16,
    pub device_id: u16,

    pub driver_name: Option<&'static str>,
    pub priority: u8,

    pub state: BindingState,
    pub lifecycle: DriverLifecycle,

    pub valid: bool,
}

impl DeviceBinding {
    pub const EMPTY: Self = Self {
        bus: 0,
        device: 0,
        function: 0,
        vendor_id: 0,
        device_id: 0,
        driver_name: None,
        priority: 0,
        state: BindingState::NoDriver,
        lifecycle: DriverLifecycle::Registered,
        valid: false,
    };

    fn from_device(
        device: &PciDevice,
        result: DeviceDriverMatch,
    ) -> Self {
        let lifecycle = match result.status {
            BindingState::Matched => DriverLifecycle::Matched,
            BindingState::ProbeRejected
            | BindingState::NoDriver => DriverLifecycle::Registered,
        };

        Self {
            bus: device.bus,
            device: device.device,
            function: device.function,
            vendor_id: device.vendor_id,
            device_id: device.device_id,
            driver_name: result.driver_name,
            priority: result.priority,
            state: result.status,
            lifecycle,
            valid: true,
        }
    }
}

#[derive(Clone, Copy)]
pub struct DeviceDriverMatch {
    pub driver_name: Option<&'static str>,
    pub status: BindingState,
    pub priority: u8,
}

impl DeviceDriverMatch {
    const fn no_driver() -> Self {
        Self {
            driver_name: None,
            status: BindingState::NoDriver,
            priority: 0,
        }
    }
}

#[derive(Clone, Copy)]
struct DriverCandidate {
    index: usize,
    priority: u8,
    specificity: u8,
}

impl DriverCandidate {
    const fn is_better_than(self, other: Self) -> bool {
        if self.priority != other.priority {
            return self.priority > other.priority;
        }

        self.specificity > other.specificity
    }
}

// ============================================================
// DRIVER MATCHING
// ============================================================

pub fn find_driver(device: &PciDevice) -> DeviceDriverMatch {
    let mut best_compatible: Option<DriverCandidate> = None;
    let mut best_rejected: Option<DriverCandidate> = None;

    for (index, driver) in DRIVERS.iter().enumerate() {
        if !driver.matcher.matches(device) {
            continue;
        }

        let candidate = DriverCandidate {
            index,
            priority: driver.priority,
            specificity: driver.matcher.specificity(),
        };

        match (driver.probe)(device) {
            ProbeResult::Compatible => {
                let replace = match best_compatible {
                    Some(current) => candidate.is_better_than(current),
                    None => true,
                };

                if replace {
                    best_compatible = Some(candidate);
                }
            }

            ProbeResult::Rejected => {
                let replace = match best_rejected {
                    Some(current) => candidate.is_better_than(current),
                    None => true,
                };

                if replace {
                    best_rejected = Some(candidate);
                }
            }
        }
    }

    if let Some(candidate) = best_compatible {
        let driver = &DRIVERS[candidate.index];

        return DeviceDriverMatch {
            driver_name: Some(driver.name),
            status: BindingState::Matched,
            priority: driver.priority,
        };
    }

    if let Some(candidate) = best_rejected {
        let driver = &DRIVERS[candidate.index];

        return DeviceDriverMatch {
            driver_name: Some(driver.name),
            status: BindingState::ProbeRejected,
            priority: driver.priority,
        };
    }

    DeviceDriverMatch::no_driver()
}

// ============================================================
// BINDING REGISTRY
// ============================================================

struct BindingRegistry {
    entries: [DeviceBinding; MAX_PCI_DEVICES],
    count: usize,
}

impl BindingRegistry {
    const fn new() -> Self {
        Self {
            entries: [DeviceBinding::EMPTY; MAX_PCI_DEVICES],
            count: 0,
        }
    }
}

static BINDINGS: spin::Mutex<BindingRegistry> =
    spin::Mutex::new(BindingRegistry::new());

// ============================================================
// LIFECYCLE ERRORS
// ============================================================

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LifecycleError {
    BindingNotFound,
    NoCompatibleDriver,
    InvalidTransition,
}

impl LifecycleError {
    pub const fn name(self) -> &'static str {
        match self {
            Self::BindingNotFound => "BINDING NOT FOUND",
            Self::NoCompatibleDriver => "NO COMPATIBLE DRIVER",
            Self::InvalidTransition => "INVALID STATE TRANSITION",
        }
    }
}

// ============================================================
// BINDING API
// ============================================================

pub fn bind_devices(inventory: &PciInventory) -> usize {
    let mut registry = BINDINGS.lock();

    registry.count = 0;

    let limit = core::cmp::min(
        inventory.count,
        MAX_PCI_DEVICES,
    );

    for index in 0..limit {
        let device = &inventory.devices[index];
        let result = find_driver(device);

        registry.entries[index] =
            DeviceBinding::from_device(device, result);

        registry.count += 1;
    }

    for index in registry.count..MAX_PCI_DEVICES {
        registry.entries[index] = DeviceBinding::EMPTY;
    }

    registry.count
}

pub fn get_binding(index: usize) -> Option<DeviceBinding> {
    let registry = BINDINGS.lock();

    if index >= registry.count {
        return None;
    }

    let binding = registry.entries[index];

    if binding.valid {
        Some(binding)
    } else {
        None
    }
}

pub fn find_binding(
    bus: u8,
    device: u8,
    function: u8,
) -> Option<DeviceBinding> {
    let registry = BINDINGS.lock();

    for index in 0..registry.count {
        let binding = registry.entries[index];

        if binding.valid
            && binding.bus == bus
            && binding.device == device
            && binding.function == function
        {
            return Some(binding);
        }
    }

    None
}

pub fn binding_count() -> usize {
    BINDINGS.lock().count
}

// ============================================================
// LIFECYCLE MANAGEMENT
// ============================================================


/// Matched -> Initializing
pub fn begin_initialization(
    bus: u8,
    device: u8,
    function: u8,
) -> Result<(), LifecycleError> {
    let mut registry = BINDINGS.lock();

    let binding = find_binding_mut(
        &mut registry,
        bus,
        device,
        function,
    )?;

    if binding.state != BindingState::Matched {
        return Err(LifecycleError::NoCompatibleDriver);
    }

    if binding.lifecycle != DriverLifecycle::Matched {
        return Err(LifecycleError::InvalidTransition);
    }

    binding.lifecycle = DriverLifecycle::Initializing;

    Ok(())
}

/// Initializing -> Initialized
pub fn mark_initialized(
    bus: u8,
    device: u8,
    function: u8,
) -> Result<(), LifecycleError> {
    let mut registry = BINDINGS.lock();

    let binding = find_binding_mut(
        &mut registry,
        bus,
        device,
        function,
    )?;

    if binding.state != BindingState::Matched {
        return Err(LifecycleError::NoCompatibleDriver);
    }

    if binding.lifecycle != DriverLifecycle::Initializing {
        return Err(LifecycleError::InvalidTransition);
    }

    binding.lifecycle = DriverLifecycle::Initialized;

    Ok(())
}

/// Initializing -> Failed
pub fn mark_failed(
    bus: u8,
    device: u8,
    function: u8,
) -> Result<(), LifecycleError> {
    let mut registry = BINDINGS.lock();

    let binding = find_binding_mut(
        &mut registry,
        bus,
        device,
        function,
    )?;

    if binding.state != BindingState::Matched {
        return Err(LifecycleError::NoCompatibleDriver);
    }

    if binding.lifecycle != DriverLifecycle::Initializing {
        return Err(LifecycleError::InvalidTransition);
    }

    binding.lifecycle = DriverLifecycle::Failed;

    Ok(())
}

/// Initialized / Failed -> Matched
pub fn reset_binding(
    bus: u8,
    device: u8,
    function: u8,
) -> Result<(), LifecycleError> {
    let mut registry = BINDINGS.lock();

    let binding = find_binding_mut(
        &mut registry,
        bus,
        device,
        function,
    )?;

    if binding.state != BindingState::Matched {
        return Err(LifecycleError::NoCompatibleDriver);
    }

    if !matches!(
        binding.lifecycle,
        DriverLifecycle::Initialized | DriverLifecycle::Failed
    ) {
        return Err(LifecycleError::InvalidTransition);
    }

    binding.lifecycle = DriverLifecycle::Matched;

    Ok(())
}


/// Initializing -> Matched
/// Used when driver initialization is not implemented yet.
pub fn cancel_initialization(
    bus: u8,
    device: u8,
    function: u8,
) -> Result<(), LifecycleError> {
    let mut registry = BINDINGS.lock();

    let binding = find_binding_mut(
        &mut registry,
        bus,
        device,
        function,
    )?;

    if binding.state != BindingState::Matched {
        return Err(LifecycleError::NoCompatibleDriver);
    }

    if binding.lifecycle != DriverLifecycle::Initializing {
        return Err(LifecycleError::InvalidTransition);
    }

    binding.lifecycle = DriverLifecycle::Matched;

    Ok(())
}

// ============================================================
// DRIVER INITIALIZATION MANAGER
// ============================================================

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DriverInitStatus {
    Success,
    NotImplemented,
    HardwareError,
}

pub fn initialize_driver(
    bus: u8,
    device: u8,
    function: u8,
) -> Result<DriverInitStatus, LifecycleError> {
    // Find the device binding.
    let binding = find_binding(bus, device, function)
        .ok_or(LifecycleError::BindingNotFound)?;

    if binding.state != BindingState::Matched {
        return Err(LifecycleError::NoCompatibleDriver);
    }

    // Find the corresponding PCI device.
    let pci_device = {
        let inventory = unsafe {
            &*core::ptr::addr_of!(super::PCI_INVENTORY)
        };

        let mut found = None;

        for index in 0..inventory.count {
            let candidate = &inventory.devices[index];

            if candidate.bus == bus
                && candidate.device == device
                && candidate.function == function
            {
                found = Some(*candidate);
                break;
            }
        }

        found.ok_or(LifecycleError::BindingNotFound)?
    };

    // Find the registered driver.
    let driver = DRIVERS
        .iter()
        .find(|driver| {
            Some(driver.name) == binding.driver_name
                && driver.matcher.matches(&pci_device)
        })
        .ok_or(LifecycleError::NoCompatibleDriver)?;

    // Move to Initializing.
    begin_initialization(bus, device, function)?;

    // Call the driver's initialization function.
    match (driver.initialize)(&pci_device) {
        DriverInitResult::Success => {
            mark_initialized(bus, device, function)?;
            Ok(DriverInitStatus::Success)
        }

        DriverInitResult::NotImplemented => {
            cancel_initialization(bus, device, function)?;
            Ok(DriverInitStatus::NotImplemented)
        }

        DriverInitResult::HardwareError => {
            mark_failed(bus, device, function)?;
            Ok(DriverInitStatus::HardwareError)
        }
    }
}


fn find_binding_mut(
    registry: &mut BindingRegistry,
    bus: u8,
    device: u8,
    function: u8,
) -> Result<&mut DeviceBinding, LifecycleError> {
    for index in 0..registry.count {
        let binding = &mut registry.entries[index];

        if binding.valid
            && binding.bus == bus
            && binding.device == device
            && binding.function == function
        {
            return Ok(binding);
        }
    }

    Err(LifecycleError::BindingNotFound)
}

// ============================================================
// STATISTICS
// ============================================================

#[derive(Clone, Copy)]
pub struct BindingStatistics {
    pub total: usize,
    pub matched: usize,
    pub rejected: usize,
    pub without_driver: usize,
    pub initialized: usize,
    pub failed: usize,
    pub initializing: usize,
}

pub fn binding_statistics() -> BindingStatistics {
    let registry = BINDINGS.lock();

    let mut stats = BindingStatistics {
        total: registry.count,
        matched: 0,
        rejected: 0,
        without_driver: 0,
        initialized: 0,
        initializing: 0,
        failed: 0,
    };

    for index in 0..registry.count {
        let binding = registry.entries[index];

        match binding.state {
            BindingState::Matched => stats.matched += 1,
            BindingState::ProbeRejected => stats.rejected += 1,
            BindingState::NoDriver => stats.without_driver += 1,
        }

        match binding.lifecycle {
            DriverLifecycle::Initialized => stats.initialized += 1,
            DriverLifecycle::Failed => stats.failed += 1,
            DriverLifecycle::Initializing => stats.initializing += 1,
            _ => {}
        }
    }

    stats
}


// ============================================================
// DRIVER MANAGER OUTPUT
// ============================================================

pub fn match_devices(
    console: &mut Console<'_>,
    inventory: &PciInventory,
) {
    let total = bind_devices(inventory);
    let stats = binding_statistics();

    console.write("\nPCI Driver Manager:\n", GREEN);

    console.write("  Registered drivers: ", GRAY);
    write_decimal(console, DRIVERS.len() as u64);
    console.write("\n", WHITE);

    console.write("  Binding records: ", GRAY);
    write_decimal(console, total as u64);
    console.write("\n", WHITE);

    console.write("\nDriver registry:\n", GREEN);

    for driver in DRIVERS {
        console.write("  ", GRAY);
        console.write(driver.name, WHITE);

        console.write("  Priority: ", GRAY);
        write_decimal(console, driver.priority as u64);

        console.write("  REGISTERED\n", GREEN);
    }

    console.write("\nDevice bindings:\n", GREEN);

    for index in 0..total {
        let Some(binding) = get_binding(index) else {
            continue;
        };

        console.write("  PCI ", GRAY);
        write_decimal(console, binding.bus as u64);
        console.write(":", GRAY);
        write_decimal(console, binding.device as u64);
        console.write(".", GRAY);
        write_decimal(console, binding.function as u64);

        console.write("  ", WHITE);
        write_hex16(console, binding.vendor_id);
        console.write(":", GRAY);
        write_hex16(console, binding.device_id);

        console.write("  ", WHITE);

        match binding.state {
            BindingState::Matched => console.write("MATCHED", GREEN),
            BindingState::ProbeRejected => {
                console.write("PROBE REJECTED", RED)
            }
            BindingState::NoDriver => {
                console.write("NO DRIVER", YELLOW)
            }
        }

        if let Some(name) = binding.driver_name {
            console.write("  ", WHITE);
            console.write(name, WHITE);
        }

        console.write("  [", GRAY);
        console.write(binding.lifecycle.name(), GREEN);
        console.write("]\n", GRAY);
    }

    console.write("\nBinding statistics:\n", GREEN);

    console.write("  Total records: ", GRAY);
    write_decimal(console, stats.total as u64);
    console.write("\n", WHITE);

    console.write("  Matched: ", GRAY);
    write_decimal(console, stats.matched as u64);
    console.write("\n", WHITE);

    console.write("  Probe rejected: ", GRAY);
    write_decimal(console, stats.rejected as u64);
    console.write("\n", WHITE);

    console.write("  Without driver: ", GRAY);
    write_decimal(console, stats.without_driver as u64);
    console.write("\n", WHITE);

    console.write("  Initialized: ", GRAY);
    write_decimal(console, stats.initialized as u64);
    console.write("\n", WHITE);

    console.write("  Initializing: ", GRAY);
    write_decimal(console, stats.initializing as u64);
    console.write("\n", WHITE);

    console.write("  Failed: ", GRAY);
    write_decimal(console, stats.failed as u64);
    console.write("\n", WHITE);

    console.write("  Hardware initialization: ", GRAY);
    console.write("NOT STARTED\n", YELLOW);
}

// ============================================================
// NUMBER FORMATTING
// ============================================================

fn write_decimal(
    console: &mut Console<'_>,
    mut number: u64,
) {
    if number == 0 {
        console.write("0", WHITE);
        return;
    }

    let mut buffer = [0u8; 20];
    let mut index = buffer.len();

    while number > 0 {
        index -= 1;
        buffer[index] = b'0' + (number % 10) as u8;
        number /= 10;
    }

    if let Ok(text) = core::str::from_utf8(&buffer[index..]) {
        console.write(text, WHITE);
    }
}

fn write_hex16(
    console: &mut Console<'_>,
    value: u16,
) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";

    console.write("0x", GRAY);

    let mut buffer = [0u8; 4];

    for index in 0..4 {
        let shift = (3 - index) * 4;
        buffer[index] = HEX[((value >> shift) & 0xF) as usize];
    }

    if let Ok(text) = core::str::from_utf8(&buffer) {
        console.write(text, WHITE);
    }
}
