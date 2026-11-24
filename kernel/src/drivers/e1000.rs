use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{fence, AtomicU64, Ordering};

use x86_64::VirtAddr;
use x86_64::structures::paging::PageTableFlags;

use crate::task::preempt::PreemptMutex;

use crate::drivers::registry::{DeviceClass, DriverStatus, KernelDriver, NetworkDevice};
use crate::hardware::PciNetworkDescriptor;
use crate::interrupts::timer;
use crate::memory::PhysicalFrameAllocator;
use crate::memory::vmm::{PAGE_SIZE, PhysicalMemoryMapping};

const E1000_MMIO_VIRT_BASE: u64 = 0x0000_5000_0000_0000;
const MAX_MMIO_MAP_BYTES: u64 = 256 * 1024;

const RX_DESC_COUNT: usize = 32;
const TX_DESC_COUNT: usize = 16;
const PACKET_BUF_SIZE: usize = 2048;

// Intel 82540EM Register Offsets
const REG_CTRL: usize = 0x0000;
const REG_STATUS: usize = 0x0008;
const REG_EECD: usize = 0x0010;
const REG_EERD: usize = 0x0014;
const REG_ICR: usize = 0x00C0;
const REG_IMS: usize = 0x00D0;
const REG_IMC: usize = 0x00D8;
const REG_RCTL: usize = 0x0100;
const REG_TCTL: usize = 0x0400;
const REG_TIPG: usize = 0x0410;
const REG_RDBAL: usize = 0x2800;
const REG_RDBAH: usize = 0x2804;
const REG_RDLEN: usize = 0x2808;
const REG_RDH: usize = 0x2810;
const REG_RDT: usize = 0x2818;
const REG_TDBAL: usize = 0x3800;
const REG_TDBAH: usize = 0x3804;
const REG_TDLEN: usize = 0x3808;
const REG_TDH: usize = 0x3810;
const REG_TDT: usize = 0x3818;
const REG_MTA: usize = 0x5200;
const REG_RAL0: usize = 0x5400;
const REG_RAH0: usize = 0x5404;

// CTRL flags
const CTRL_ASDE: u32 = 1 << 5;
const CTRL_SLU: u32 = 1 << 6;
const CTRL_RST: u32 = 1 << 26;

// STATUS flags
const STATUS_LU: u32 = 1 << 1;

// RCTL flags
const RCTL_EN: u32 = 1 << 1;
const RCTL_UPE: u32 = 1 << 3;
const RCTL_MPE: u32 = 1 << 4;
const RCTL_BAM: u32 = 1 << 15;
const RCTL_SECRC: u32 = 1 << 26;

// TCTL flags
const TCTL_EN: u32 = 1 << 1;
const TCTL_PSP: u32 = 1 << 3;
const TCTL_CT_SHIFT: u32 = 4;
const TCTL_COLD_SHIFT: u32 = 12;

// TX Descriptor command/status
const TX_CMD_EOP: u8 = 1 << 0;
const TX_CMD_IFCS: u8 = 1 << 1;
const TX_CMD_RS: u8 = 1 << 3;
const DESC_STATUS_DD: u8 = 1 << 0;
const RX_STATUS_EOP: u8 = 1 << 1;

#[repr(C, align(16))]
#[derive(Clone, Copy)]
struct RxDescriptor {
    addr: u64,
    length: u16,
    checksum: u16,
    status: u8,
    errors: u8,
    special: u16,
}

#[repr(C, align(16))]
#[derive(Clone, Copy)]
struct TxDescriptor {
    addr: u64,
    length: u16,
    cso: u8,
    cmd: u8,
    status: u8,
    css: u8,
    special: u16,
}

#[derive(Clone, Copy)]
pub struct E1000Stats {
    pub initialized: bool,
    pub link_up: bool,
    pub mac: [u8; 6],
    pub mmio_phys: u64,
    pub mmio_virt: u64,
    pub irq_line: u8,
    pub tx_packets: u64,
    pub rx_packets: u64,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    pub tx_errors: u64,
}

pub struct E1000Device {
    initialized: bool,
    status: DriverStatus,
    mmio_phys: u64,
    mmio_virt: u64,
    irq_line: u8,
    mac: [u8; 6],
    rx_ring_virt: u64,
    tx_ring_virt: u64,
    rx_buf_virts: [u64; RX_DESC_COUNT],
    tx_buf_virts: [u64; TX_DESC_COUNT],
    rx_cur: usize,
    tx_cur: usize,
    tx_packets: u64,
    rx_packets: u64,
    tx_bytes: u64,
    rx_bytes: u64,
    tx_errors: u64,
}

impl E1000Device {
    pub const fn empty() -> Self {
        Self {
            initialized: false,
            status: DriverStatus::NotPresent,
            mmio_phys: 0,
            mmio_virt: 0,
            irq_line: 0,
            mac: [0; 6],
            rx_ring_virt: 0,
            tx_ring_virt: 0,
            rx_buf_virts: [0; RX_DESC_COUNT],
            tx_buf_virts: [0; TX_DESC_COUNT],
            rx_cur: 0,
            tx_cur: 0,
            tx_packets: 0,
            rx_packets: 0,
            tx_bytes: 0,
            rx_bytes: 0,
            tx_errors: 0,
        }
    }

    #[inline]
    fn read_reg(&self, offset: usize) -> u32 {
        unsafe { read_volatile((self.mmio_virt + offset as u64) as *const u32) }
    }

    #[inline]
    fn write_reg(&self, offset: usize, value: u32) {
        unsafe { write_volatile((self.mmio_virt + offset as u64) as *mut u32, value) }
    }

    fn read_eeprom_word(&self, addr: u8) -> Option<u16> {
        self.write_reg(REG_EERD, 1 | ((addr as u32) << 8));
        for _ in 0..50_000 {
            let val = self.read_reg(REG_EERD);
            if (val & (1 << 4)) != 0 {
                return Some(((val >> 16) & 0xFFFF) as u16);
            }
            core::hint::spin_loop();
        }
        None
    }

    fn detect_mac(&self) -> [u8; 6] {
        // Attempt EEPROM read first (standard 82540EM).
        let _ = self.read_reg(REG_EECD);
        if let (Some(w0), Some(w1), Some(w2)) = (
            self.read_eeprom_word(0),
            self.read_eeprom_word(1),
            self.read_eeprom_word(2),
        ) {
            let mac = [
                (w0 & 0xFF) as u8,
                (w0 >> 8) as u8,
                (w1 & 0xFF) as u8,
                (w1 >> 8) as u8,
                (w2 & 0xFF) as u8,
                (w2 >> 8) as u8,
            ];
            if mac != [0; 6] && mac != [0xFF; 6] {
                return mac;
            }
        }

        // Fallback: read from RAL0 / RAH0 pre-populated by QEMU.
        let ral = self.read_reg(REG_RAL0);
        let rah = self.read_reg(REG_RAH0);
        let mac = [
            (ral & 0xFF) as u8,
            ((ral >> 8) & 0xFF) as u8,
            ((ral >> 16) & 0xFF) as u8,
            ((ral >> 24) & 0xFF) as u8,
            (rah & 0xFF) as u8,
            ((rah >> 8) & 0xFF) as u8,
        ];
        if mac != [0; 6] {
            mac
        } else {
            // Standard QEMU default fallback MAC if unreadable.
            [0x52, 0x54, 0x00, 0x12, 0x34, 0x56]
        }
    }

    pub fn init_hardware(
        &mut self,
        desc: &PciNetworkDescriptor,
        mapping: &PhysicalMemoryMapping,
        allocator: &mut PhysicalFrameAllocator<'_>,
    ) -> Result<(), &'static str> {
        if !desc.valid || desc.mmio_base == 0 {
            self.status = DriverStatus::NotPresent;
            return Err("Intel e1000 device not present");
        }

        let map_size = desc.mmio_size.clamp(0x20000, MAX_MMIO_MAP_BYTES);
        let phys_page_base = desc.mmio_base & !0xFFF;
        let phys_offset_in_page = desc.mmio_base & 0xFFF;
        let page_count = map_size.div_ceil(PAGE_SIZE);

        let mmio_flags = PageTableFlags::PRESENT
            | PageTableFlags::WRITABLE
            | PageTableFlags::NO_CACHE
            | PageTableFlags::WRITE_THROUGH
            | PageTableFlags::NO_EXECUTE;

        for page_idx in 0..page_count {
            let vaddr = VirtAddr::new(E1000_MMIO_VIRT_BASE + page_idx * PAGE_SIZE);
            let paddr = phys_page_base + page_idx * PAGE_SIZE;
            if mapping.translate_virtual_address(vaddr).is_none() {
                mapping
                    .map_page(allocator, vaddr, paddr, mmio_flags)
                    .map_err(|_| "Failed to map e1000 MMIO BAR")?;
            }
        }

        self.mmio_phys = desc.mmio_base;
        self.mmio_virt = E1000_MMIO_VIRT_BASE + phys_offset_in_page;
        self.irq_line = desc.irq_line;

        // Read MAC before resetting device (or restore RAL/RAH after reset).
        let mac = self.detect_mac();
        self.mac = mac;

        // Disable interrupts before reset.
        self.write_reg(REG_IMC, 0xFFFF_FFFF);
        let _ = self.read_reg(REG_ICR);

        // Reset e1000 controller.
        let ctrl = self.read_reg(REG_CTRL);
        self.write_reg(REG_CTRL, ctrl | CTRL_RST);
        timer::sleep_ms(10);
        for _ in 0..100_000 {
            if (self.read_reg(REG_CTRL) & CTRL_RST) == 0 {
                break;
            }
            core::hint::spin_loop();
        }

        // Disable interrupts again post-reset.
        self.write_reg(REG_IMC, 0xFFFF_FFFF);
        self.write_reg(REG_IMS, 0);
        let _ = self.read_reg(REG_ICR);

        // Set Link Up and Auto-Speed Detection.
        let mut ctrl = self.read_reg(REG_CTRL);
        ctrl |= CTRL_SLU | CTRL_ASDE;
        ctrl &= !(1 << 3); // Clear LRST
        ctrl &= !(1 << 31); // Clear PHY_RST
        self.write_reg(REG_CTRL, ctrl);

        // Clear Multicast Table Array (128 dwords).
        for idx in 0..128 {
            self.write_reg(REG_MTA + idx * 4, 0);
        }

        // Program Receive Address Register 0 (MAC + Address Valid bit 31).
        let ral = (mac[0] as u32)
            | ((mac[1] as u32) << 8)
            | ((mac[2] as u32) << 16)
            | ((mac[3] as u32) << 24);
        let rah = (mac[4] as u32) | ((mac[5] as u32) << 8) | (1 << 31);
        self.write_reg(REG_RAL0, ral);
        self.write_reg(REG_RAH0, rah);

        // Allocate RX descriptor ring (1 frame = 4096B >= 32 * 16B).
        let rx_ring_phys = allocator
            .allocate_frame()
            .ok_or("Failed to allocate RX ring frame")?;
        let rx_ring_virt = mapping
            .translate_frame(rx_ring_phys)
            .ok_or("Failed to translate RX ring frame")?
            .as_u64();
        unsafe {
            core::ptr::write_bytes(rx_ring_virt as *mut u8, 0, PAGE_SIZE as usize);
        }

        // Allocate 16 frames for 32 RX buffers (2048 bytes each).
        for i in 0..(RX_DESC_COUNT / 2) {
            let buf_frame_phys = allocator
                .allocate_frame()
                .ok_or("Failed to allocate RX buffer frame")?;
            let buf_frame_virt = mapping
                .translate_frame(buf_frame_phys)
                .ok_or("Failed to translate RX buffer frame")?
                .as_u64();
            unsafe {
                core::ptr::write_bytes(buf_frame_virt as *mut u8, 0, PAGE_SIZE as usize);
            }

            let d0_idx = i * 2;
            let d1_idx = i * 2 + 1;

            self.rx_buf_virts[d0_idx] = buf_frame_virt;
            self.rx_buf_virts[d1_idx] = buf_frame_virt + PACKET_BUF_SIZE as u64;

            let ring_ptr = rx_ring_virt as *mut RxDescriptor;
            unsafe {
                write_volatile(
                    ring_ptr.add(d0_idx),
                    RxDescriptor {
                        addr: buf_frame_phys,
                        length: 0,
                        checksum: 0,
                        status: 0,
                        errors: 0,
                        special: 0,
                    },
                );
                write_volatile(
                    ring_ptr.add(d1_idx),
                    RxDescriptor {
                        addr: buf_frame_phys + PACKET_BUF_SIZE as u64,
                        length: 0,
                        checksum: 0,
                        status: 0,
                        errors: 0,
                        special: 0,
                    },
                );
            }
        }

        self.rx_ring_virt = rx_ring_virt;
        self.rx_cur = 0;

        self.write_reg(REG_RDBAL, (rx_ring_phys & 0xFFFF_FFFF) as u32);
        self.write_reg(REG_RDBAH, (rx_ring_phys >> 32) as u32);
        self.write_reg(REG_RDLEN, (RX_DESC_COUNT * core::mem::size_of::<RxDescriptor>()) as u32);
        self.write_reg(REG_RDH, 0);
        self.write_reg(REG_RDT, (RX_DESC_COUNT - 1) as u32);

        // Allocate TX descriptor ring (1 frame).
        let tx_ring_phys = allocator
            .allocate_frame()
            .ok_or("Failed to allocate TX ring frame")?;
        let tx_ring_virt = mapping
            .translate_frame(tx_ring_phys)
            .ok_or("Failed to translate TX ring frame")?
            .as_u64();
        unsafe {
            core::ptr::write_bytes(tx_ring_virt as *mut u8, 0, PAGE_SIZE as usize);
        }

        // Allocate 8 frames for 16 TX buffers (2048 bytes each).
        for i in 0..(TX_DESC_COUNT / 2) {
            let buf_frame_phys = allocator
                .allocate_frame()
                .ok_or("Failed to allocate TX buffer frame")?;
            let buf_frame_virt = mapping
                .translate_frame(buf_frame_phys)
                .ok_or("Failed to translate TX buffer frame")?
                .as_u64();
            unsafe {
                core::ptr::write_bytes(buf_frame_virt as *mut u8, 0, PAGE_SIZE as usize);
            }

            let d0_idx = i * 2;
            let d1_idx = i * 2 + 1;

            self.tx_buf_virts[d0_idx] = buf_frame_virt;
            self.tx_buf_virts[d1_idx] = buf_frame_virt + PACKET_BUF_SIZE as u64;

            let ring_ptr = tx_ring_virt as *mut TxDescriptor;
            unsafe {
                write_volatile(
                    ring_ptr.add(d0_idx),
                    TxDescriptor {
                        addr: buf_frame_phys,
                        length: 0,
                        cso: 0,
                        cmd: 0,
                        status: DESC_STATUS_DD,
                        css: 0,
                        special: 0,
                    },
                );
                write_volatile(
                    ring_ptr.add(d1_idx),
                    TxDescriptor {
                        addr: buf_frame_phys + PACKET_BUF_SIZE as u64,
                        length: 0,
                        cso: 0,
                        cmd: 0,
                        status: DESC_STATUS_DD,
                        css: 0,
                        special: 0,
                    },
                );
            }
        }

        self.tx_ring_virt = tx_ring_virt;
        self.tx_cur = 0;

        self.write_reg(REG_TDBAL, (tx_ring_phys & 0xFFFF_FFFF) as u32);
        self.write_reg(REG_TDBAH, (tx_ring_phys >> 32) as u32);
        self.write_reg(REG_TDLEN, (TX_DESC_COUNT * core::mem::size_of::<TxDescriptor>()) as u32);
        self.write_reg(REG_TDH, 0);
        self.write_reg(REG_TDT, 0);

        // Enable RX: 2048B buffers, broadcast accept, promiscuous off (UPE/MPE optional, keep BAM+SECRC).
        let rctl = RCTL_EN | RCTL_BAM | RCTL_SECRC | RCTL_UPE | RCTL_MPE;
        self.write_reg(REG_RCTL, rctl);

        // Enable TX: pad short packets, CT=15, COLD=64.
        let tctl = TCTL_EN | TCTL_PSP | (15 << TCTL_CT_SHIFT) | (64 << TCTL_COLD_SHIFT);
        self.write_reg(REG_TCTL, tctl);
        self.write_reg(REG_TIPG, 10 | (8 << 10) | (6 << 20));

        // Enable RX/link hardware interrupts (RXT0 | RXO | RXDMT0 | LSC) for IOAPIC WorkLoop delivery.
        self.write_reg(REG_IMS, (1 << 7) | (1 << 6) | (1 << 4) | (1 << 2));
        let _ = self.read_reg(REG_ICR);

        fence(Ordering::SeqCst);
        self.initialized = true;
        self.status = DriverStatus::Active;
        E1000_MMIO_VIRT_FAST.store(self.mmio_virt, Ordering::Release);
        Ok(())
    }

    pub fn stats(&self) -> E1000Stats {
        E1000Stats {
            initialized: self.initialized,
            link_up: self.link_up(),
            mac: self.mac,
            mmio_phys: self.mmio_phys,
            mmio_virt: self.mmio_virt,
            irq_line: self.irq_line,
            tx_packets: self.tx_packets,
            rx_packets: self.rx_packets,
            tx_bytes: self.tx_bytes,
            rx_bytes: self.rx_bytes,
            tx_errors: self.tx_errors,
        }
    }
}

impl KernelDriver for E1000Device {
    fn driver_name(&self) -> &'static str {
        "Intel 82540EM (e1000)"
    }

    fn device_class(&self) -> DeviceClass {
        DeviceClass::Network
    }

    fn status(&self) -> DriverStatus {
        self.status
    }
}

impl NetworkDevice for E1000Device {
    fn mac_address(&self) -> [u8; 6] {
        self.mac
    }

    fn link_up(&self) -> bool {
        if !self.initialized {
            return false;
        }
        (self.read_reg(REG_STATUS) & STATUS_LU) != 0
    }

    fn send_frame(&mut self, frame: &[u8]) -> Result<(), &'static str> {
        if !self.initialized {
            return Err("e1000 driver is not initialized");
        }
        if frame.is_empty() || frame.len() > 1522 {
            return Err("Invalid Ethernet frame length");
        }

        let idx = self.tx_cur;
        let ring_ptr = self.tx_ring_virt as *mut TxDescriptor;
        let buf_virt = self.tx_buf_virts[idx] as *mut u8;

        // Wait if previous descriptor hasn't finished yet.
        for _ in 0..200_000 {
            let desc = unsafe { read_volatile(ring_ptr.add(idx)) };
            if (desc.status & DESC_STATUS_DD) != 0 {
                break;
            }
            core::hint::spin_loop();
        }

        unsafe {
            core::ptr::copy_nonoverlapping(frame.as_ptr(), buf_virt, frame.len());
            let mut desc = read_volatile(ring_ptr.add(idx));
            desc.length = frame.len() as u16;
            desc.cso = 0;
            desc.css = 0;
            desc.special = 0;
            desc.cmd = TX_CMD_EOP | TX_CMD_IFCS | TX_CMD_RS;
            desc.status = 0;
            write_volatile(ring_ptr.add(idx), desc);
        }

        fence(Ordering::SeqCst);
        let next_tail = (idx + 1) % TX_DESC_COUNT;
        self.tx_cur = next_tail;
        self.write_reg(REG_TDT, next_tail as u32);

        // Poll for TX descriptor Done bit.
        let mut completed = false;
        for _ in 0..500_000 {
            let desc = unsafe { read_volatile(ring_ptr.add(idx)) };
            if (desc.status & DESC_STATUS_DD) != 0 {
                completed = true;
                break;
            }
            core::hint::spin_loop();
        }

        if completed {
            self.tx_packets += 1;
            self.tx_bytes += frame.len() as u64;
            Ok(())
        } else {
            self.tx_errors += 1;
            Err("e1000 TX timeout")
        }
    }

    fn poll_frame(&mut self, out: &mut [u8]) -> Option<usize> {
        if !self.initialized {
            return None;
        }

        let idx = self.rx_cur;
        let ring_ptr = self.rx_ring_virt as *mut RxDescriptor;
        let desc = unsafe { read_volatile(ring_ptr.add(idx)) };

        if (desc.status & DESC_STATUS_DD) == 0 {
            return None;
        }

        let mut copied = None;
        if (desc.status & RX_STATUS_EOP) != 0 && desc.errors == 0 && desc.length > 0 {
            let len = (desc.length as usize).min(out.len()).min(PACKET_BUF_SIZE);
            let src = self.rx_buf_virts[idx] as *const u8;
            unsafe {
                core::ptr::copy_nonoverlapping(src, out.as_mut_ptr(), len);
            }
            self.rx_packets += 1;
            self.rx_bytes += len as u64;
            copied = Some(len);
        }

        // Return descriptor to hardware.
        unsafe {
            let mut clean = desc;
            clean.length = 0;
            clean.checksum = 0;
            clean.status = 0;
            clean.errors = 0;
            clean.special = 0;
            write_volatile(ring_ptr.add(idx), clean);
        }
        fence(Ordering::SeqCst);
        self.write_reg(REG_RDT, idx as u32);
        self.rx_cur = (idx + 1) % RX_DESC_COUNT;

        copied
    }
}

pub static E1000_DRIVER: PreemptMutex<E1000Device> = PreemptMutex::new(E1000Device::empty());
static E1000_MMIO_VIRT_FAST: AtomicU64 = AtomicU64::new(0);
static E1000_IRQ_COUNT: AtomicU64 = AtomicU64::new(0);

/// Lock-free Top-Half interrupt handler invoked from IOAPIC vector before LAPIC EOI.
pub fn irq_top_half() -> bool {
    let mmio_virt = E1000_MMIO_VIRT_FAST.load(Ordering::Acquire);
    if mmio_virt == 0 {
        return false;
    }
    let icr = unsafe { read_volatile((mmio_virt + REG_ICR as u64) as *const u32) };
    if icr != 0 {
        E1000_IRQ_COUNT.fetch_add(1, Ordering::Relaxed);
        crate::drivers::iokit::signal_workloop(crate::drivers::iokit::WorkLoopKind::Network);
        true
    } else {
        false
    }
}

pub fn irq_count() -> u64 {
    E1000_IRQ_COUNT.load(Ordering::Relaxed)
}

pub fn init(
    desc: &PciNetworkDescriptor,
    mapping: Option<&PhysicalMemoryMapping>,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> Result<[u8; 6], &'static str> {
    let mapping = mapping.ok_or("PhysicalMemoryMapping unavailable for e1000")?;
    let mut nic = E1000_DRIVER.lock();
    nic.init_hardware(desc, mapping, allocator)?;
    Ok(nic.mac_address())
}

pub fn is_ready() -> bool {
    E1000_DRIVER.lock().initialized
}

pub fn stats() -> E1000Stats {
    E1000_DRIVER.lock().stats()
}

pub fn send_frame(frame: &[u8]) -> Result<(), &'static str> {
    E1000_DRIVER.lock().send_frame(frame)
}

pub fn poll_frame(out: &mut [u8]) -> Option<usize> {
    E1000_DRIVER.lock().poll_frame(out)
}
