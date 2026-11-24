use crate::console::Console;
use crate::hardware::{
    IdeChannelDescriptor,
    IdeControllerDescriptor,
    IdeControllerDiscoveryList,
    MAX_IDE_CONTROLLERS,
};
use crate::storage::ata::{
    self,
    diagnostics,
    AtaChannelDevices,
    AtaDevice,
    AtaDeviceType,
    AtaIoError,
    AtaProbeStatus,
};
use crate::storage::block::{
    BlockDevice,
    BlockDeviceError,
    DEFAULT_SECTOR_SIZE,
};

#[derive(Clone, Copy)]
pub struct IdeRuntimeController {
    pub info: IdeControllerDescriptor,
    pub primary_devices: AtaChannelDevices,
    pub secondary_devices: AtaChannelDevices,
}

impl IdeRuntimeController {
    pub const EMPTY: Self = Self {
        info: IdeControllerDescriptor::EMPTY,
        primary_devices: AtaChannelDevices::EMPTY,
        secondary_devices: AtaChannelDevices::EMPTY,
    };
}

struct IdeRuntimeRegistry {
    controllers: [IdeRuntimeController; MAX_IDE_CONTROLLERS],
    count: usize,
    overflow: usize,
}

impl IdeRuntimeRegistry {
    const fn new() -> Self {
        Self {
            controllers: [IdeRuntimeController::EMPTY; MAX_IDE_CONTROLLERS],
            count: 0,
            overflow: 0,
        }
    }

    fn clear(&mut self) {
        self.controllers = [IdeRuntimeController::EMPTY; MAX_IDE_CONTROLLERS];
        self.count = 0;
        self.overflow = 0;
    }

    fn register(&mut self, controller: IdeRuntimeController) -> bool {
        if self.count >= MAX_IDE_CONTROLLERS {
            self.overflow += 1;
            return false;
        }

        self.controllers[self.count] = controller;
        self.count += 1;
        true
    }
}

static IDE_RUNTIME: spin::Mutex<IdeRuntimeRegistry> =
    spin::Mutex::new(IdeRuntimeRegistry::new());

/// Clears all previously registered IDE controllers.
pub fn clear_runtime_state() {
    IDE_RUNTIME.lock().clear();
}

/// Probes ATA devices for each discovered IDE controller and stores runtime state.
pub fn register_discovered_controllers(
    console: &mut Console<'_>,
    discovered: &IdeControllerDiscoveryList,
) {
    clear_runtime_state();

    for index in 0..discovered.count {
        let info = discovered.controllers[index];
        let (primary_devices, secondary_devices) = diagnostics::display_results(
            console,
            &info.primary,
            &info.secondary,
        );

        let controller = IdeRuntimeController {
            info,
            primary_devices,
            secondary_devices,
        };

        if !IDE_RUNTIME.lock().register(controller) {
            console.write("  Storage IDE runtime registry is full!\n", (220, 90, 90));
        }
    }

    if discovered.overflow > 0 {
        let mut lock = IDE_RUNTIME.lock();
        lock.overflow = lock.overflow.saturating_add(discovered.overflow);
    }
}

/// Stores an IDE controller and its ATA detection results directly.
pub fn register_controller(
    info: IdeControllerDescriptor,
    primary_devices: AtaChannelDevices,
    secondary_devices: AtaChannelDevices,
) -> bool {
    let controller = IdeRuntimeController {
        info,
        primary_devices,
        secondary_devices,
    };

    IDE_RUNTIME.lock().register(controller)
}

/// Returns a copy of a previously registered controller.
pub fn get_controller(index: usize) -> Option<IdeRuntimeController> {
    let registry = IDE_RUNTIME.lock();

    if index >= registry.count {
        return None;
    }

    Some(registry.controllers[index])
}

/// Returns the number of registered IDE controllers.
pub fn controller_count() -> usize {
    IDE_RUNTIME.lock().count
}

/// Returns the number of controllers that could not be stored.
pub fn controller_overflow() -> usize {
    IDE_RUNTIME.lock().overflow
}

/// ATA PIO device exposed through the generic block-device interface.
#[derive(Clone, Copy)]
pub struct IdeBlockDevice {
    controller_index: usize,
    secondary: bool,
    slave: bool,
    channel: IdeChannelDescriptor,
    device: AtaDevice,
}

impl IdeBlockDevice {
    /// Creates an adapter for a selected device on the primary channel.
    pub fn primary(
        controller_index: usize,
        slave: bool,
    ) -> Result<Self, BlockDeviceError> {
        Self::from_slot(controller_index, false, slave)
    }

    /// Creates an adapter for a selected device on the secondary channel.
    pub fn secondary(
        controller_index: usize,
        slave: bool,
    ) -> Result<Self, BlockDeviceError> {
        Self::from_slot(controller_index, true, slave)
    }

    /// Creates an adapter for any IDE controller/channel/drive slot.
    pub fn from_slot(
        controller_index: usize,
        secondary: bool,
        slave: bool,
    ) -> Result<Self, BlockDeviceError> {
        let controller =
            get_controller(controller_index).ok_or(BlockDeviceError::DeviceNotReady)?;

        let channel = if secondary {
            controller.info.secondary
        } else {
            controller.info.primary
        };

        let devices = if secondary {
            controller.secondary_devices
        } else {
            controller.primary_devices
        };

        let device = if slave { devices.slave } else { devices.master };

        if !channel.valid {
            return Err(BlockDeviceError::DeviceNotReady);
        }

        if device.status != AtaProbeStatus::Present
            || device.device_type != Some(AtaDeviceType::Ata)
            || !device.lba_supported
        {
            return Err(BlockDeviceError::DeviceNotReady);
        }

        if device.logical_sector_size == 0 {
            return Err(BlockDeviceError::InvalidSectorSize);
        }

        Ok(Self {
            controller_index,
            secondary,
            slave,
            channel,
            device,
        })
    }

    /// Finds the first available ATA LBA disk across all registered controllers and channels.
    pub fn first_ata_disk() -> Result<Self, BlockDeviceError> {
        let count = controller_count();
        for controller_index in 0..count {
            for secondary in [false, true] {
                for slave in [false, true] {
                    if let Ok(disk) = Self::from_slot(controller_index, secondary, slave) {
                        return Ok(disk);
                    }
                }
            }
        }
        Err(BlockDeviceError::DeviceNotReady)
    }

    pub fn controller_index(&self) -> usize {
        self.controller_index
    }

    pub fn is_secondary(&self) -> bool {
        self.secondary
    }

    pub fn is_slave(&self) -> bool {
        self.slave
    }

    pub fn channel_info(&self) -> &IdeChannelDescriptor {
        &self.channel
    }

    pub fn device_info(&self) -> &AtaDevice {
        &self.device
    }

    fn map_error(error: AtaIoError) -> BlockDeviceError {
        match error {
            AtaIoError::InvalidChannel
            | AtaIoError::DeviceNotReady
            | AtaIoError::UnsupportedDevice => BlockDeviceError::DeviceNotReady,
            AtaIoError::LbaOutOfRange => BlockDeviceError::SectorOutOfRange,
            AtaIoError::Timeout => BlockDeviceError::Timeout,
            AtaIoError::DeviceError => BlockDeviceError::IoError,
        }
    }
}

impl BlockDevice for IdeBlockDevice {
    fn sector_size(&self) -> u32 {
        self.device.logical_sector_size
    }

    fn sector_count(&self) -> u64 {
        self.device.capacity_sectors
    }

    fn max_transfer_sectors(&self) -> usize {
        64
    }

    fn read_sector(
        &mut self,
        lba: u64,
        buffer: &mut [u8],
    ) -> Result<(), BlockDeviceError> {
        if self.sector_size() != DEFAULT_SECTOR_SIZE {
            return Err(BlockDeviceError::InvalidSectorSize);
        }

        if buffer.len() != DEFAULT_SECTOR_SIZE as usize {
            return Err(BlockDeviceError::InvalidBufferSize);
        }

        if lba >= self.sector_count() {
            return Err(BlockDeviceError::SectorOutOfRange);
        }

        let sector_buffer: &mut [u8; 512] = buffer
            .try_into()
            .map_err(|_| BlockDeviceError::InvalidBufferSize)?;

        ata::read_sector(&self.channel, &self.device, self.slave, lba, sector_buffer)
            .map_err(Self::map_error)
    }

    fn write_sector(
        &mut self,
        lba: u64,
        buffer: &[u8],
    ) -> Result<(), BlockDeviceError> {
        if self.sector_size() != DEFAULT_SECTOR_SIZE {
            return Err(BlockDeviceError::InvalidSectorSize);
        }

        if buffer.len() != DEFAULT_SECTOR_SIZE as usize {
            return Err(BlockDeviceError::InvalidBufferSize);
        }

        if lba >= self.sector_count() {
            return Err(BlockDeviceError::SectorOutOfRange);
        }

        let sector_buffer: &[u8; 512] = buffer
            .try_into()
            .map_err(|_| BlockDeviceError::InvalidBufferSize)?;

        ata::write_sectors(&self.channel, &self.device, self.slave, lba, 1, sector_buffer)
            .map_err(Self::map_error)
    }

    fn read_sectors(
        &mut self,
        start_lba: u64,
        sector_count: usize,
        buffer: &mut [u8],
    ) -> Result<(), BlockDeviceError> {
        if sector_count == 0 {
            return if buffer.is_empty() {
                Ok(())
            } else {
                Err(BlockDeviceError::InvalidBufferSize)
            };
        }
        if self.sector_size() != DEFAULT_SECTOR_SIZE {
            return Err(BlockDeviceError::InvalidSectorSize);
        }
        let expected = sector_count
            .checked_mul(DEFAULT_SECTOR_SIZE as usize)
            .ok_or(BlockDeviceError::InvalidBufferSize)?;
        if buffer.len() != expected {
            return Err(BlockDeviceError::InvalidBufferSize);
        }
        let end = start_lba
            .checked_add(sector_count as u64)
            .ok_or(BlockDeviceError::SectorOutOfRange)?;
        if end > self.sector_count() {
            return Err(BlockDeviceError::SectorOutOfRange);
        }
        ata::read_sectors(&self.channel, &self.device, self.slave, start_lba, sector_count, buffer)
            .map_err(Self::map_error)
    }

    fn write_sectors(
        &mut self,
        start_lba: u64,
        sector_count: usize,
        buffer: &[u8],
    ) -> Result<(), BlockDeviceError> {
        if sector_count == 0 {
            return if buffer.is_empty() {
                Ok(())
            } else {
                Err(BlockDeviceError::InvalidBufferSize)
            };
        }
        if self.sector_size() != DEFAULT_SECTOR_SIZE {
            return Err(BlockDeviceError::InvalidSectorSize);
        }
        let expected = sector_count
            .checked_mul(DEFAULT_SECTOR_SIZE as usize)
            .ok_or(BlockDeviceError::InvalidBufferSize)?;
        if buffer.len() != expected {
            return Err(BlockDeviceError::InvalidBufferSize);
        }
        let end = start_lba
            .checked_add(sector_count as u64)
            .ok_or(BlockDeviceError::SectorOutOfRange)?;
        if end > self.sector_count() {
            return Err(BlockDeviceError::SectorOutOfRange);
        }
        ata::write_sectors(&self.channel, &self.device, self.slave, start_lba, sector_count, buffer)
            .map_err(Self::map_error)
    }

    fn flush(&mut self) -> Result<(), BlockDeviceError> {
        ata::flush_cache(&self.channel, &self.device, self.slave).map_err(Self::map_error)
    }
}
