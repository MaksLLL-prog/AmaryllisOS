use bootloader_api::info::{FrameBufferInfo, PixelFormat};
use x86_64::VirtAddr;

use crate::freebsd_kpi::Mtx;
use crate::hardware::PciDisplayDescriptor;
use crate::memory::vmm::PhysicalMemoryMapping;

/// FreeBSD `vt_efifb` / `efifb`-inspired UEFI GOP & linear framebuffer driver.
#[derive(Clone, Copy)]
pub struct GopModeSnapshot {
    pub initialized: bool,
    pub width: usize,
    pub height: usize,
    pub stride_pixels: usize,
    pub bytes_per_pixel: usize,
    pub bits_per_pixel: usize,
    pub pitch_bytes: usize,
    pub fb_virt: u64,
    pub fb_phys: u64,
    pub fb_size: usize,
    pub pixel_format_code: u8, // 0 = RGB, 1 = BGR, 2 = U8, 3 = Unknown
    pub red_mask: u32,
    pub green_mask: u32,
    pub blue_mask: u32,
    pub reserved_mask: u32,
    pub pci_present: bool,
    pub pci_vendor: u16,
    pub pci_device: u16,
    pub pci_bus: u8,
    pub pci_dev: u8,
    pub pci_func: u8,
    pub pci_lfb_phys: u64,
    pub pci_mmio_phys: u64,
}

impl GopModeSnapshot {
    pub const EMPTY: Self = Self {
        initialized: false,
        width: 0,
        height: 0,
        stride_pixels: 0,
        bytes_per_pixel: 0,
        bits_per_pixel: 0,
        pitch_bytes: 0,
        fb_virt: 0,
        fb_phys: 0,
        fb_size: 0,
        pixel_format_code: 3,
        red_mask: 0,
        green_mask: 0,
        blue_mask: 0,
        reserved_mask: 0,
        pci_present: false,
        pci_vendor: 0,
        pci_device: 0,
        pci_bus: 0,
        pci_dev: 0,
        pci_func: 0,
        pci_lfb_phys: 0,
        pci_mmio_phys: 0,
    };

    pub const fn format_name(&self) -> &'static str {
        match self.pixel_format_code {
            0 => "RGBx8888",
            1 => "BGRx8888 (GOP/BGA)",
            2 => "Gray8",
            _ => "Bitmask/Other",
        }
    }
}

static GOP_STATE: Mtx<GopModeSnapshot> = Mtx::new("efifb", GopModeSnapshot::EMPTY);

pub fn init(
    fb_virt: u64,
    fb_size: usize,
    fb_info: &FrameBufferInfo,
    display_pci: &PciDisplayDescriptor,
    mapping: Option<&PhysicalMemoryMapping>,
) -> GopModeSnapshot {
    let translated_phys = if fb_virt != 0 {
        mapping
            .and_then(|m| m.translate_virtual_address(VirtAddr::new(fb_virt)))
            .map(|t| t.physical_address)
            .unwrap_or(0)
    } else {
        0
    };

    let fb_phys = if translated_phys != 0 {
        translated_phys
    } else if display_pci.valid && display_pci.lfb_phys != 0 {
        display_pci.lfb_phys
    } else {
        0
    };

    let (pixel_format_code, red_mask, green_mask, blue_mask, reserved_mask) =
        match fb_info.pixel_format {
            PixelFormat::Rgb => (0, 0x0000_00FF, 0x0000_FF00, 0x00FF_0000, 0xFF00_0000),
            PixelFormat::Bgr => (1, 0x00FF_0000, 0x0000_FF00, 0x0000_00FF, 0xFF00_0000),
            PixelFormat::U8 => (2, 0x0000_00FF, 0x0000_00FF, 0x0000_00FF, 0x0000_0000),
            PixelFormat::Unknown {
                red_position,
                green_position,
                blue_position,
            } => (
                3,
                0xFFu32.wrapping_shl(red_position as u32),
                0xFFu32.wrapping_shl(green_position as u32),
                0xFFu32.wrapping_shl(blue_position as u32),
                0,
            ),
            _ => (3, 0x00FF_0000, 0x0000_FF00, 0x0000_00FF, 0xFF00_0000),
        };

    let snap = GopModeSnapshot {
        initialized: fb_virt != 0 && fb_info.width > 0 && fb_info.height > 0,
        width: fb_info.width,
        height: fb_info.height,
        stride_pixels: fb_info.stride,
        bytes_per_pixel: fb_info.bytes_per_pixel,
        bits_per_pixel: fb_info.bytes_per_pixel * 8,
        pitch_bytes: fb_info.stride * fb_info.bytes_per_pixel,
        fb_virt,
        fb_phys,
        fb_size,
        pixel_format_code,
        red_mask,
        green_mask,
        blue_mask,
        reserved_mask,
        pci_present: display_pci.valid,
        pci_vendor: display_pci.vendor_id,
        pci_device: display_pci.device_id,
        pci_bus: display_pci.bus,
        pci_dev: display_pci.device,
        pci_func: display_pci.function,
        pci_lfb_phys: display_pci.lfb_phys,
        pci_mmio_phys: display_pci.mmio_phys,
    };

    *GOP_STATE.lock() = snap;
    snap
}

pub fn is_ready() -> bool {
    GOP_STATE.lock().initialized
}

pub fn snapshot() -> GopModeSnapshot {
    *GOP_STATE.lock()
}

pub fn try_set_bga_mode(width: u16, height: u16, bpp: u16) -> bool {
    use x86_64::instructions::port::Port;
    unsafe {
        let mut idx = Port::<u16>::new(0x01CE);
        let mut dat = Port::<u16>::new(0x01CF);
        idx.write(0x0000); // VBE_DISPI_INDEX_ID
        let id = dat.read();
        if !(0xB0C0..=0xB0C5).contains(&id) {
            return false;
        }
        idx.write(0x0004); // VBE_DISPI_INDEX_ENABLE
        dat.write(0x0000); // disable
        idx.write(0x0001); // XRES
        dat.write(width);
        idx.write(0x0002); // YRES
        dat.write(height);
        idx.write(0x0003); // BPP
        dat.write(bpp);
        idx.write(0x0006); // VIRT_WIDTH
        dat.write(width);
        idx.write(0x0008); // X_OFFSET
        dat.write(0);
        idx.write(0x0009); // Y_OFFSET
        dat.write(0);
        idx.write(0x0004); // ENABLE | LFB_ENABLED
        dat.write(0x0041);
        true
    }
}
