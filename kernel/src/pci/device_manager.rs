
use super::driver::{
    self,
    BindingState,
    DeviceBinding,
    DriverLifecycle,
};

// ============================================================
// PCI DEVICE ADDRESS
// ============================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PciAddress {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
}

impl PciAddress {
    pub const fn new(
        bus: u8,
        device: u8,
        function: u8,
    ) -> Self {
        Self {
            bus,
            device,
            function,
        }
    }

    pub const fn is_valid(&self) -> bool {
        self.device < 32 && self.function < 8
    }
}

// ============================================================
// DEVICE INFORMATION
// ============================================================

#[derive(Clone, Copy)]
pub struct DeviceInfo {
    pub address: PciAddress,

    pub vendor_id: u16,
    pub device_id: u16,

    pub driver_name: Option<&'static str>,
    pub driver_priority: u8,

    pub binding_state: BindingState,
    pub lifecycle: DriverLifecycle,
}

impl DeviceInfo {
    fn from_binding(binding: DeviceBinding) -> Self {
        Self {
            address: PciAddress::new(
                binding.bus,
                binding.device,
                binding.function,
            ),

            vendor_id: binding.vendor_id,
            device_id: binding.device_id,

            driver_name: binding.driver_name,
            driver_priority: binding.priority,

            binding_state: binding.state,
            lifecycle: binding.lifecycle,
        }
    }
}

// ============================================================
// DEVICE QUERY
// ============================================================

#[derive(Clone, Copy)]
pub struct DeviceQuery {
    pub vendor_id: Option<u16>,
    pub device_id: Option<u16>,
    pub binding_state: Option<BindingState>,
    pub lifecycle: Option<DriverLifecycle>,
}

impl DeviceQuery {
    pub const fn all() -> Self {
        Self {
            vendor_id: None,
            device_id: None,
            binding_state: None,
            lifecycle: None,
        }
    }

    pub const fn vendor(vendor_id: u16) -> Self {
        Self {
            vendor_id: Some(vendor_id),
            device_id: None,
            binding_state: None,
            lifecycle: None,
        }
    }

    pub const fn device(
        vendor_id: u16,
        device_id: u16,
    ) -> Self {
        Self {
            vendor_id: Some(vendor_id),
            device_id: Some(device_id),
            binding_state: None,
            lifecycle: None,
        }
    }

    pub const fn matches(&self, info: &DeviceInfo) -> bool {
        matches_u16(self.vendor_id, info.vendor_id)
            && matches_u16(self.device_id, info.device_id)
            && matches_binding(
                self.binding_state,
                info.binding_state,
            )
            && matches_lifecycle(
                self.lifecycle,
                info.lifecycle,
            )
    }
}

const fn matches_u16(
    expected: Option<u16>,
    actual: u16,
) -> bool {
    match expected {
        Some(value) => value == actual,
        None => true,
    }
}

const fn matches_binding(
    expected: Option<BindingState>,
    actual: BindingState,
) -> bool {
    match expected {
        Some(value) => value as u8 == actual as u8,
        None => true,
    }
}

const fn matches_lifecycle(
    expected: Option<DriverLifecycle>,
    actual: DriverLifecycle,
) -> bool {
    match expected {
        Some(value) => value as u8 == actual as u8,
        None => true,
    }
}

// ============================================================
// DEVICE MANAGER ERRORS
// ============================================================

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DeviceManagerError {
    InvalidAddress,
    DeviceNotFound,
}

// ============================================================
// DEVICE LOOKUP
// ============================================================

/// Количество устройств в реестре.
pub fn count() -> usize {
    driver::binding_count()
}

/// Поиск устройства по PCI-адресу.
pub fn find(
    address: PciAddress,
) -> Result<DeviceInfo, DeviceManagerError> {
    if !address.is_valid() {
        return Err(DeviceManagerError::InvalidAddress);
    }

    let binding = driver::find_binding(
        address.bus,
        address.device,
        address.function,
    )
    .ok_or(DeviceManagerError::DeviceNotFound)?;

    Ok(DeviceInfo::from_binding(binding))
}

/// Поиск по Vendor ID и Device ID.
pub fn find_by_id(
    vendor_id: u16,
    device_id: u16,
) -> Option<DeviceInfo> {
    let total = driver::binding_count();

    for index in 0..total {
        let Some(binding) = driver::get_binding(index) else {
            continue;
        };

        if binding.vendor_id == vendor_id
            && binding.device_id == device_id
        {
            return Some(DeviceInfo::from_binding(binding));
        }
    }

    None
}

/// Поиск первого устройства указанного производителя.
pub fn find_by_vendor(
    vendor_id: u16,
) -> Option<DeviceInfo> {
    let total = driver::binding_count();

    for index in 0..total {
        let Some(binding) = driver::get_binding(index) else {
            continue;
        };

        if binding.vendor_id == vendor_id {
            return Some(DeviceInfo::from_binding(binding));
        }
    }

    None
}

// ============================================================
// DEVICE ENUMERATION
// ============================================================

/// Заполняет переданный массив информацией об устройствах.
///
/// Возвращает количество записанных элементов.
/// Если массив меньше количества устройств, лишние записи
/// пропускаются.
pub fn enumerate(output: &mut [DeviceInfo]) -> usize {
    query(DeviceQuery::all(), output)
}

/// Поиск устройств с фильтрами.
///
/// Возвращает количество записанных элементов.
/// Динамическая память не используется.
pub fn query(
    filter: DeviceQuery,
    output: &mut [DeviceInfo],
) -> usize {
    let total = driver::binding_count();
    let mut written = 0usize;

    for index in 0..total {
        if written >= output.len() {
            break;
        }

        let Some(binding) = driver::get_binding(index) else {
            continue;
        };

        let info = DeviceInfo::from_binding(binding);

        if filter.matches(&info) {
            output[written] = info;
            written += 1;
        }
    }

    written
}

// ============================================================
// DEVICE STATE
// ============================================================

pub fn is_initialized(address: PciAddress) -> bool {
    match find(address) {
        Ok(info) => {
            info.lifecycle == DriverLifecycle::Initialized
        }
        Err(_) => false,
    }
}

pub fn has_driver(address: PciAddress) -> bool {
    match find(address) {
        Ok(info) => {
            info.binding_state == BindingState::Matched
                && info.driver_name.is_some()
        }
        Err(_) => false,
    }
}
