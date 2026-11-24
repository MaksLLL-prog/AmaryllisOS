pub const MAX_IDE_CONTROLLERS: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum IdeChannelMode {
    Compatibility,
    Native,
    Unsupported,
}

impl IdeChannelMode {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Compatibility => "COMPATIBILITY",
            Self::Native => "NATIVE PCI",
            Self::Unsupported => "UNSUPPORTED",
        }
    }
}

#[derive(Clone, Copy)]
pub struct IdeChannelDescriptor {
    pub mode: IdeChannelMode,
    pub command_base: u16,
    pub control_base: u16,
    pub bus_master_base: Option<u16>,
    pub valid: bool,
}

impl IdeChannelDescriptor {
    pub const EMPTY: Self = Self {
        mode: IdeChannelMode::Unsupported,
        command_base: 0,
        control_base: 0,
        bus_master_base: None,
        valid: false,
    };
}

#[derive(Clone, Copy)]
pub struct IdeControllerDescriptor {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub primary: IdeChannelDescriptor,
    pub secondary: IdeChannelDescriptor,
    pub bus_master_supported: bool,
}

impl IdeControllerDescriptor {
    pub const EMPTY: Self = Self {
        bus: 0,
        device: 0,
        function: 0,
        vendor_id: 0,
        device_id: 0,
        primary: IdeChannelDescriptor::EMPTY,
        secondary: IdeChannelDescriptor::EMPTY,
        bus_master_supported: false,
    };
}

#[derive(Clone, Copy)]
pub struct IdeControllerDiscoveryList {
    pub controllers: [IdeControllerDescriptor; MAX_IDE_CONTROLLERS],
    pub count: usize,
    pub overflow: usize,
}

impl IdeControllerDiscoveryList {
    pub const EMPTY: Self = Self {
        controllers: [IdeControllerDescriptor::EMPTY; MAX_IDE_CONTROLLERS],
        count: 0,
        overflow: 0,
    };

    pub fn push(&mut self, controller: IdeControllerDescriptor) -> bool {
        if self.count >= MAX_IDE_CONTROLLERS {
            self.overflow += 1;
            return false;
        }

        self.controllers[self.count] = controller;
        self.count += 1;
        true
    }
}

#[derive(Clone, Copy)]
pub struct PciNetworkDescriptor {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub irq_line: u8,
    pub mmio_base: u64,
    pub mmio_size: u64,
    pub io_base: u16,
    pub valid: bool,
}

impl PciNetworkDescriptor {
    pub const EMPTY: Self = Self {
        bus: 0,
        device: 0,
        function: 0,
        vendor_id: 0,
        device_id: 0,
        irq_line: 0,
        mmio_base: 0,
        mmio_size: 0,
        io_base: 0,
        valid: false,
    };
}

#[derive(Clone, Copy)]
pub struct PciAudioDescriptor {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub irq_line: u8,
    pub nambar_io: u16,
    pub nabmbar_io: u16,
    pub valid: bool,
}

impl PciAudioDescriptor {
    pub const EMPTY: Self = Self {
        bus: 0,
        device: 0,
        function: 0,
        vendor_id: 0,
        device_id: 0,
        irq_line: 0,
        nambar_io: 0,
        nabmbar_io: 0,
        valid: false,
    };
}

#[derive(Clone, Copy)]
pub struct PciAhciDescriptor {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub irq_line: u8,
    pub abar_phys: u64,
    pub abar_size: u64,
    pub valid: bool,
}

impl PciAhciDescriptor {
    pub const EMPTY: Self = Self {
        bus: 0,
        device: 0,
        function: 0,
        vendor_id: 0,
        device_id: 0,
        irq_line: 0,
        abar_phys: 0,
        abar_size: 0,
        valid: false,
    };
}

#[derive(Clone, Copy)]
pub struct PciVirtioBlkDescriptor {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub irq_line: u8,
    pub io_base: u16,
    pub mmio_base: u64,
    pub mmio_size: u64,
    pub is_modern: bool,
    pub valid: bool,
}

impl PciVirtioBlkDescriptor {
    pub const EMPTY: Self = Self {
        bus: 0,
        device: 0,
        function: 0,
        vendor_id: 0,
        device_id: 0,
        irq_line: 0,
        io_base: 0,
        mmio_base: 0,
        mmio_size: 0,
        is_modern: false,
        valid: false,
    };
}

#[derive(Clone, Copy)]
pub struct HardwareDiscovery {
    pub ide_controllers: IdeControllerDiscoveryList,
    pub network: PciNetworkDescriptor,
    pub audio: PciAudioDescriptor,
    pub ahci: PciAhciDescriptor,
    pub virtio_blk: PciVirtioBlkDescriptor,
}

impl HardwareDiscovery {
    pub const EMPTY: Self = Self {
        ide_controllers: IdeControllerDiscoveryList::EMPTY,
        network: PciNetworkDescriptor::EMPTY,
        audio: PciAudioDescriptor::EMPTY,
        ahci: PciAhciDescriptor::EMPTY,
        virtio_blk: PciVirtioBlkDescriptor::EMPTY,
    };
}
