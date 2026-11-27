use core::arch::asm;
use core::ptr::{copy_nonoverlapping, read_volatile, write_volatile};
use core::sync::atomic::{fence, AtomicBool, AtomicU16, AtomicU64, AtomicU8, Ordering};

use crate::freebsd_kpi::{
    bus_alloc_resource_any, bus_dma_tag_create, bus_dmamap_sync, bus_dmamem_alloc,
    bus_setup_intr, bus_space_barrier, bus_space_read_1, bus_space_read_2, bus_space_read_4,
    bus_space_write_1, bus_space_write_2, bus_space_write_4, device_add_pci_child,
    device_probe_and_attach, pci_enable_busmaster, with_device_mut, BusDmaMap, BusDmaTag,
    BusSpaceHandle, DeviceId, DeviceT, DriverT, Mtx, BUS_DMASYNC_POSTREAD,
    BUS_DMASYNC_PREWRITE, BUS_DMA_NOWAIT, BUS_DMA_ZERO, BUS_PROBE_SPECIFIC, ENXIO,
    SYS_RES_IOPORT, SYS_RES_IRQ, SYS_RES_MEMORY,
};
use crate::hardware::PciRealtekNetDescriptor;
use crate::interrupts::timer;
use crate::memory::vmm::{PhysicalMemoryMapping, PAGE_SIZE};
use crate::memory::PhysicalFrameAllocator;
use crate::pci;

/// FreeBSD `if_re` / `if_rl`-style Realtek Ethernet driver:
/// - `RTL8168 / RTL8169 / RTL8111 / RTL8125 / RTL8136` (C+ 64-entry DMA rings)
/// - `RTL8139` (`0x8139`, 4-slot TX + 32 KiB RX ring buffer)

const RE_DESC_COUNT: usize = 32;
const RE_PKT_BUF_SIZE: usize = 2048;

// Shared Realtek registers
const RL_IDR0: usize = 0x00;
const RL_CR: usize = 0x37;
const RL_TPPOLL: usize = 0x38;
const RL_IMR_8139: usize = 0x3C;
const RL_ISR_8139: usize = 0x3E;
const RL_TXCFG: usize = 0x40;
const RL_RXCFG: usize = 0x44;
const RL_9346CR: usize = 0x50;
const RL_PHYSTATUS: usize = 0x6C;

// RTL8168/8169 C+ Registers
const RE_IMR: usize = 0x3C;
const RE_ISR: usize = 0x3E;
const RE_TX_RING_LO: usize = 0x20;
const RE_TX_RING_HI: usize = 0x24;
const RE_RX_MAX_SIZE: usize = 0xDA;
const RE_CPLUS_CMD: usize = 0xE0;
const RE_RX_RING_LO: usize = 0xE4;
const RE_RX_RING_HI: usize = 0xE8;
const RE_MTPS: usize = 0xEC;

// RTL8139-specific registers
const RL8139_TSD0: usize = 0x10;
const RL8139_TSAD0: usize = 0x20;
const RL8139_RBSTART: usize = 0x30;
const RL8139_CAPR: usize = 0x38;
const RL8139_CBR: usize = 0x3A;
const RL8139_CONFIG1: usize = 0x52;

// Command register bits
const CR_RST: u8 = 1 << 4;
const CR_RE: u8 = 1 << 3;
const CR_TE: u8 = 1 << 2;
const CR_BUFE: u8 = 1 << 0;

// C+ descriptor flags
const DESC_OWN: u32 = 1 << 31;
const DESC_EOR: u32 = 1 << 30;
const DESC_FS: u32 = 1 << 29;
const DESC_LS: u32 = 1 << 28;
const DESC_LEN_MASK: u32 = 0x3FFF;

const ISR_ROK: u16 = 1 << 0;
const ISR_RER: u16 = 1 << 1;
const ISR_TOK: u16 = 1 << 2;
const ISR_TER: u16 = 1 << 3;
const ISR_RDU: u16 = 1 << 4;
const ISR_LINK: u16 = 1 << 5;
const RE_ISR_MASK: u16 = ISR_ROK | ISR_RER | ISR_TOK | ISR_TER | ISR_RDU | ISR_LINK;

#[repr(C, align(16))]
#[derive(Clone, Copy)]
struct ReDesc {
    cmd_sts: u32,
    vlan: u32,
    buf_lo: u32,
    buf_hi: u32,
}

#[derive(Clone, Copy)]
pub struct ReNicStats {
    pub initialized: bool,
    pub is_rtl8139: bool,
    pub link_up: bool,
    pub mac: [u8; 6],
    pub mmio_phys: u64,
    pub mmio_virt: u64,
    pub io_base: u16,
    pub irq_line: u8,
    pub irq_gsi: Option<u32>,
    pub irq_vector: Option<u8>,
    pub device_id: u16,
    pub tx_packets: u64,
    pub rx_packets: u64,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    pub tx_errors: u64,
}

impl ReNicStats {
    pub const EMPTY: Self = Self {
        initialized: false,
        is_rtl8139: false,
        link_up: false,
        mac: [0; 6],
        mmio_phys: 0,
        mmio_virt: 0,
        io_base: 0,
        irq_line: 0,
        irq_gsi: None,
        irq_vector: None,
        device_id: 0,
        tx_packets: 0,
        rx_packets: 0,
        tx_bytes: 0,
        rx_bytes: 0,
        tx_errors: 0,
    };
}

struct ReSoftc {
    snap: ReNicStats,
    reg: BusSpaceHandle,
    tag: BusDmaTag,
    tx_ring_map: BusDmaMap,
    rx_ring_map: BusDmaMap,
    tx_buf_maps: [Option<BusDmaMap>; 4],
    rx_buf_maps: [Option<BusDmaMap>; RE_DESC_COUNT],
    rl_rx_ring_map: BusDmaMap, // 32 KiB + 2 KiB wrap for RTL8139
    rl_rx_offset: usize,
    tx_cur: usize,
    rx_cur: usize,
}

impl ReSoftc {
    const EMPTY: Self = Self {
        snap: ReNicStats::EMPTY,
        reg: BusSpaceHandle::EMPTY,
        tag: BusDmaTag::EMPTY,
        tx_ring_map: BusDmaMap::empty(),
        rx_ring_map: BusDmaMap::empty(),
        tx_buf_maps: [const { None }; 4],
        rx_buf_maps: [const { None }; RE_DESC_COUNT],
        rl_rx_ring_map: BusDmaMap::empty(),
        rl_rx_offset: 0,
        tx_cur: 0,
        rx_cur: 0,
    };

    #[inline]
    fn r8(&self, off: usize) -> u8 {
        bus_space_read_1(&self.reg, off)
    }

    #[inline]
    fn w8(&self, off: usize, v: u8) {
        bus_space_write_1(&self.reg, off, v);
    }

    #[inline]
    fn r16(&self, off: usize) -> u16 {
        bus_space_read_2(&self.reg, off)
    }

    #[inline]
    fn w16(&self, off: usize, v: u16) {
        bus_space_write_2(&self.reg, off, v);
    }

    #[inline]
    fn w32(&self, off: usize, v: u32) {
        bus_space_write_4(&self.reg, off, v);
    }

    fn reset_chip(&self) -> bool {
        self.w8(RL_CR, CR_RST);
        for _ in 0..10_000 {
            if (self.r8(RL_CR) & CR_RST) == 0 {
                return true;
            }
            core::hint::spin_loop();
        }
        false
    }

    fn read_mac(&self) -> [u8; 6] {
        let mut mac = [0u8; 6];
        for (i, slot) in mac.iter_mut().enumerate() {
            *slot = self.r8(RL_IDR0 + i);
        }
        if mac == [0; 6] || mac == [0xFF; 6] {
            [0x52, 0x54, 0x00, 0x12, 0x34, 0x81]
        } else {
            mac
        }
    }

    fn refresh_link(&mut self) -> bool {
        if !self.snap.initialized {
            return false;
        }
        if self.snap.is_rtl8139 {
            // Basic Media Status Register (0x58): bit 2 = Link Fail (1 = down)
            let msr = self.r8(0x58);
            self.snap.link_up = (msr & (1 << 2)) == 0;
        } else {
            let phystat = self.r8(RL_PHYSTATUS);
            self.snap.link_up = (phystat & (1 << 1)) != 0;
        }
        self.snap.link_up
    }

    fn send_frame(&mut self, frame: &[u8]) -> Result<(), &'static str> {
        if !self.snap.initialized {
            return Err("Realtek NIC not initialized");
        }
        if frame.is_empty() || frame.len() > 1518 {
            return Err("Invalid Ethernet frame length");
        }

        let len = frame.len().max(60);

        if self.snap.is_rtl8139 {
            let slot = self.tx_cur % 4;
            let Some(ref buf_map) = self.tx_buf_maps[slot] else {
                return Err("RTL8139 TX buffer missing");
            };
            unsafe {
                let dst = buf_map.virt_addr() as *mut u8;
                core::ptr::write_bytes(dst, 0, 64);
                copy_nonoverlapping(frame.as_ptr(), dst, frame.len());
            }
            bus_dmamap_sync(BUS_DMASYNC_PREWRITE);
            self.w32(RL8139_TSAD0 + slot * 4, buf_map.phys_addr() as u32);
            self.w32(RL8139_TSD0 + slot * 4, (len as u32) & 0x1FFF);
            bus_space_barrier();

            let mut ok = false;
            for _ in 0..50_000 {
                let tsd = bus_space_read_4(&self.reg, RL8139_TSD0 + slot * 4);
                if (tsd & (1 << 15)) != 0 {
                    ok = true;
                    break;
                }
                core::hint::spin_loop();
            }
            self.tx_cur = (slot + 1) % 4;
            if ok {
                self.snap.tx_packets += 1;
                self.snap.tx_bytes += frame.len() as u64;
                Ok(())
            } else {
                self.snap.tx_errors += 1;
                Err("RTL8139 TX timeout")
            }
        } else {
            let slot = self.tx_cur % 4;
            let Some(ref buf_map) = self.tx_buf_maps[slot] else {
                return Err("re0 TX buffer missing");
            };
            unsafe {
                let dst = buf_map.virt_addr() as *mut u8;
                core::ptr::write_bytes(dst, 0, 64);
                copy_nonoverlapping(frame.as_ptr(), dst, frame.len());
            }
            bus_dmamap_sync(BUS_DMASYNC_PREWRITE);

            let ring_ptr = self.tx_ring_map.virt_addr() as *mut ReDesc;
            let eor = if slot == 3 { DESC_EOR } else { 0 };
            let desc = ReDesc {
                cmd_sts: DESC_OWN | eor | DESC_FS | DESC_LS | ((len as u32) & DESC_LEN_MASK),
                vlan: 0,
                buf_lo: buf_map.phys_addr() as u32,
                buf_hi: (buf_map.phys_addr() >> 32) as u32,
            };
            unsafe { write_volatile(ring_ptr.add(slot), desc) };
            fence(Ordering::SeqCst);
            self.w8(RL_TPPOLL, 1 << 6); // NPQ transmit poll
            bus_space_barrier();

            let mut ok = false;
            for _ in 0..80_000 {
                let d = unsafe { read_volatile(ring_ptr.add(slot)) };
                if (d.cmd_sts & DESC_OWN) == 0 {
                    ok = true;
                    break;
                }
                core::hint::spin_loop();
            }
            self.tx_cur = (slot + 1) % 4;
            if ok {
                self.snap.tx_packets += 1;
                self.snap.tx_bytes += frame.len() as u64;
                Ok(())
            } else {
                self.snap.tx_errors += 1;
                Err("Realtek re0 TX timeout")
            }
        }
    }

    fn poll_frame(&mut self, out: &mut [u8]) -> Option<usize> {
        if !self.snap.initialized {
            return None;
        }
        if self.snap.is_rtl8139 {
            if (self.r8(RL_CR) & CR_BUFE) != 0 {
                return None;
            }
            let base = self.rl_rx_ring_map.virt_addr();
            let off = self.rl_rx_offset % 32_768;
            let hdr = unsafe { read_volatile((base + off as u64) as *const u32) };
            let status = (hdr & 0xFFFF) as u16;
            let pkt_len = ((hdr >> 16) & 0xFFFF) as usize;
            if (status & 1) == 0 || pkt_len < 4 || pkt_len > 2048 {
                let cbr = self.r16(RL8139_CBR);
                self.rl_rx_offset = cbr as usize;
                self.w16(RL8139_CAPR, cbr.wrapping_sub(16));
                return None;
            }
            let payload_len = (pkt_len.saturating_sub(4)).min(out.len()).min(RE_PKT_BUF_SIZE);
            unsafe {
                copy_nonoverlapping((base + off as u64 + 4) as *const u8, out.as_mut_ptr(), payload_len);
            }
            let next_off = (off + pkt_len + 4 + 3) & !3;
            self.rl_rx_offset = next_off % 32_768;
            self.w16(RL8139_CAPR, (self.rl_rx_offset as u16).wrapping_sub(16));
            self.snap.rx_packets += 1;
            self.snap.rx_bytes += payload_len as u64;
            Some(payload_len)
        } else {
            let idx = self.rx_cur;
            let ring_ptr = self.rx_ring_map.virt_addr() as *mut ReDesc;
            let desc = unsafe { read_volatile(ring_ptr.add(idx)) };
            if (desc.cmd_sts & DESC_OWN) != 0 {
                return None;
            }
            let mut copied = None;
            if (desc.cmd_sts & DESC_FS) != 0 && (desc.cmd_sts & DESC_LS) != 0 {
                let raw_len = (desc.cmd_sts & DESC_LEN_MASK) as usize;
                let frame_len = raw_len.saturating_sub(4).min(out.len()).min(RE_PKT_BUF_SIZE);
                if frame_len > 0 {
                    if let Some(ref bmap) = self.rx_buf_maps[idx] {
                        bus_dmamap_sync(BUS_DMASYNC_POSTREAD);
                        unsafe {
                            copy_nonoverlapping(bmap.virt_addr() as *const u8, out.as_mut_ptr(), frame_len);
                        }
                        self.snap.rx_packets += 1;
                        self.snap.rx_bytes += frame_len as u64;
                        copied = Some(frame_len);
                    }
                }
            }
            let eor = if idx + 1 == RE_DESC_COUNT { DESC_EOR } else { 0 };
            if let Some(ref bmap) = self.rx_buf_maps[idx] {
                let phys = bmap.phys_addr();
                let clean = ReDesc {
                    cmd_sts: DESC_OWN | eor | (RE_PKT_BUF_SIZE as u32),
                    vlan: 0,
                    buf_lo: phys as u32,
                    buf_hi: (phys >> 32) as u32,
                };
                unsafe { write_volatile(ring_ptr.add(idx), clean) };
                fence(Ordering::SeqCst);
            }
            self.rx_cur = (idx + 1) % RE_DESC_COUNT;
            copied
        }
    }
}

static RE_SOFTC: Mtx<ReSoftc> = Mtx::new("re0", ReSoftc::EMPTY);
static RE_READY_FAST: AtomicBool = AtomicBool::new(false);
static RE_IS_8139_FAST: AtomicU8 = AtomicU8::new(0);
static RE_MMIO_VIRT_FAST: AtomicU64 = AtomicU64::new(0);
static RE_IO_BASE_FAST: AtomicU16 = AtomicU16::new(0);
static RE_IRQ_COUNT: AtomicU64 = AtomicU64::new(0);

#[inline]
unsafe fn inw_port(port: u16) -> u16 {
    let v: u16;
    unsafe {
        asm!("in ax, dx", in("dx") port, out("ax") v, options(nomem, nostack, preserves_flags));
    }
    v
}

#[inline]
unsafe fn outw_port(port: u16, val: u16) {
    unsafe {
        asm!("out dx, ax", in("dx") port, in("ax") val, options(nomem, nostack, preserves_flags));
    }
}

fn re_irq_handler(_arg: usize) -> bool {
    if !RE_READY_FAST.load(Ordering::Acquire) {
        return false;
    }
    let mmio = RE_MMIO_VIRT_FAST.load(Ordering::Acquire);
    let io = RE_IO_BASE_FAST.load(Ordering::Acquire);
    let isr = if mmio != 0 {
        let v = unsafe { read_volatile((mmio + RE_ISR as u64) as *const u16) };
        if v != 0 {
            unsafe { write_volatile((mmio + RE_ISR as u64) as *mut u16, v) };
        }
        v
    } else if io != 0 {
        let off = if RE_IS_8139_FAST.load(Ordering::Acquire) != 0 {
            RL_ISR_8139 as u16
        } else {
            RE_ISR as u16
        };
        let v = unsafe { inw_port(io + off) };
        if v != 0 {
            unsafe { outw_port(io + off, v) };
        }
        v
    } else {
        0
    };
    if isr != 0 {
        RE_IRQ_COUNT.fetch_add(1, Ordering::Relaxed);
        crate::drivers::iokit::signal_workloop(crate::drivers::iokit::WorkLoopKind::Network);
        true
    } else {
        false
    }
}

pub fn irq_top_half() -> bool {
    re_irq_handler(0)
}

pub fn irq_count() -> u64 {
    RE_IRQ_COUNT.load(Ordering::Relaxed)
}

fn re_probe(dev: &DeviceT) -> i32 {
    if dev.vendor_id == 0x10EC
        && matches!(
            dev.device_id,
            0x8139 | 0x8161 | 0x8167 | 0x8168 | 0x8169 | 0x8136 | 0x8125 | 0x2600 | 0x3000
        )
    {
        BUS_PROBE_SPECIFIC
    } else {
        ENXIO
    }
}

fn re_attach(
    dev_id: DeviceId,
    mapping: Option<&PhysicalMemoryMapping>,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> i32 {
    let Some(dev_copy) = crate::freebsd_kpi::with_device(dev_id, |d| *d) else { return 6; };
    let dev = &dev_copy;
    let Some(map) = mapping else { return 12; };
    let pci_dev = match pci::find_pci_device(dev.bus, dev.device, dev.function) {
        Some(d) => d,
        None => return 19,
    };
    let is_8139 = dev.device_id == 0x8139;
    let irq_res = pci_enable_busmaster(&pci_dev.into_dev_hint(dev), true, true);

    // Prefer Memory BAR (BAR2 on 8168, BAR1 on 8139, fallback IO BAR0)
    let mut reg_res = None;
    for &rid in if is_8139 { &[1u8, 0u8, 2u8] } else { &[2u8, 0u8, 1u8] } {
        if let Ok(r) = bus_alloc_resource_any(dev, SYS_RES_MEMORY, rid, Some(map), allocator) {
            reg_res = Some(r);
            break;
        }
    }
    if reg_res.is_none() {
        for rid in 0..4u8 {
            if let Ok(r) = bus_alloc_resource_any(dev, SYS_RES_IOPORT, rid, Some(map), allocator) {
                reg_res = Some(r);
                break;
            }
        }
    }
    let Some(res) = reg_res else { return 6; };
    let Some(handle) = res.bus_handle else { return 6; };

    let mut sc = RE_SOFTC.lock();
    sc.reg = handle;
    sc.snap.is_rtl8139 = is_8139;
    sc.snap.device_id = dev.device_id;
    sc.snap.irq_line = irq_res;
    if res.kind == SYS_RES_MEMORY {
        sc.snap.mmio_phys = res.phys_start;
        sc.snap.mmio_virt = handle.virt_base;
    } else {
        sc.snap.io_base = res.phys_start as u16;
    }

    if !sc.reset_chip() {
        return 5;
    }
    sc.snap.mac = sc.read_mac();

    sc.tag = bus_dma_tag_create("re_dma", 256, 0xFFFF_FFFF, u64::MAX, 64 * 1024, 16, PAGE_SIZE as usize);

    // Allocate 4 TX buffers
    for i in 0..4 {
        match bus_dmamem_alloc(&sc.tag, PAGE_SIZE as usize, BUS_DMA_NOWAIT | BUS_DMA_ZERO, map, allocator) {
            Ok(m) => sc.tx_buf_maps[i] = Some(m),
            Err(_) => return 12,
        }
    }

    if is_8139 {
        let Ok(rx_map) = bus_dmamem_alloc(&sc.tag, 36 * 1024, BUS_DMA_NOWAIT | BUS_DMA_ZERO, map, allocator) else {
            return 12;
        };
        sc.w8(RL8139_CONFIG1, 0x00);
        sc.w32(RL8139_RBSTART, rx_map.phys_addr() as u32);
        sc.rl_rx_ring_map = rx_map;
        sc.w16(RL_IMR_8139, RE_ISR_MASK);
        sc.w8(RL_CR, CR_RE | CR_TE);
        // Accept broadcast + physical match + wrap
        sc.w32(RL_RXCFG, 0x0000_000F | (1 << 7));
        sc.w32(RL_TXCFG, 0x0300_0700);
    } else {
        let Ok(tx_ring) = bus_dmamem_alloc(&sc.tag, PAGE_SIZE as usize, BUS_DMA_NOWAIT | BUS_DMA_ZERO, map, allocator) else {
            return 12;
        };
        let Ok(rx_ring) = bus_dmamem_alloc(&sc.tag, PAGE_SIZE as usize, BUS_DMA_NOWAIT | BUS_DMA_ZERO, map, allocator) else {
            return 12;
        };
        let rx_ptr = rx_ring.virt_addr() as *mut ReDesc;
        for i in 0..RE_DESC_COUNT {
            let Ok(bmap) = bus_dmamem_alloc(&sc.tag, PAGE_SIZE as usize, BUS_DMA_NOWAIT | BUS_DMA_ZERO, map, allocator) else {
                return 12;
            };
            let phys = bmap.phys_addr();
            let eor = if i + 1 == RE_DESC_COUNT { DESC_EOR } else { 0 };
            unsafe {
                write_volatile(
                    rx_ptr.add(i),
                    ReDesc {
                        cmd_sts: DESC_OWN | eor | (RE_PKT_BUF_SIZE as u32),
                        vlan: 0,
                        buf_lo: phys as u32,
                        buf_hi: (phys >> 32) as u32,
                    },
                );
            }
            sc.rx_buf_maps[i] = Some(bmap);
        }
        sc.tx_ring_map = tx_ring;
        sc.rx_ring_map = rx_ring;

        sc.w8(RL_9346CR, 0xC0); // Unlock config regs
        sc.w16(RE_RX_MAX_SIZE, 0x1FFF);
        sc.w16(RE_CPLUS_CMD, 0x0020); // RX checksum/C+ mode
        sc.w8(RE_MTPS, 0x3B);
        sc.w32(RE_TX_RING_LO, sc.tx_ring_map.phys_addr() as u32);
        sc.w32(RE_TX_RING_HI, (sc.tx_ring_map.phys_addr() >> 32) as u32);
        sc.w32(RE_RX_RING_LO, sc.rx_ring_map.phys_addr() as u32);
        sc.w32(RE_RX_RING_HI, (sc.rx_ring_map.phys_addr() >> 32) as u32);
        sc.w32(RL_TXCFG, 0x0300_0700);
        sc.w32(RL_RXCFG, 0x0000_E70E);
        sc.w8(RL_CR, CR_RE | CR_TE);
        sc.w16(RE_IMR, RE_ISR_MASK);
        sc.w8(RL_9346CR, 0x00);
    }

    bus_space_barrier();
    let intr = bus_setup_intr(irq_res, "re0", re_irq_handler, dev_id).ok();
    sc.snap.irq_gsi = intr.map(|r| r.0);
    sc.snap.irq_vector = intr.map(|r| r.1);
    sc.snap.initialized = true;
    sc.refresh_link();

    RE_IS_8139_FAST.store(if is_8139 { 1 } else { 0 }, Ordering::Release);
    RE_MMIO_VIRT_FAST.store(sc.snap.mmio_virt, Ordering::Release);
    RE_IO_BASE_FAST.store(sc.snap.io_base, Ordering::Release);
    RE_READY_FAST.store(true, Ordering::Release);

    let mac = sc.snap.mac;
    drop(sc);

    with_device_mut(dev_id, |d| {
        d.bar0_phys = res.phys_start;
        d.bar0_virt = handle.virt_base;
        d.irq_line = irq_res;
        d.irq_gsi = intr.map(|r| r.0);
        d.irq_vector = intr.map(|r| r.1);
        d.set_desc(&alloc::format!(
            "Realtek {:04x} {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            dev.device_id, mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
        ));
    });
    let _ = timer::ticks();
    0
}

trait PciDevHintExt {
    fn into_dev_hint(&self, d: &DeviceT) -> DeviceT;
}
impl PciDevHintExt for pci::PciDevice {
    fn into_dev_hint(&self, d: &DeviceT) -> DeviceT {
        *d
    }
}

fn re_detach(_dev_id: DeviceId) -> i32 {
    RE_READY_FAST.store(false, Ordering::Release);
    let mut sc = RE_SOFTC.lock();
    if sc.snap.initialized {
        sc.w8(RL_CR, 0);
        sc.snap.initialized = false;
    }
    0
}

pub static RE_NIC_DRIVER: DriverT = DriverT {
    name: "re",
    probe: re_probe,
    attach: re_attach,
    detach: re_detach,
};

pub fn init(
    desc: &PciRealtekNetDescriptor,
    mapping: Option<&PhysicalMemoryMapping>,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> Result<[u8; 6], &'static str> {
    if !desc.valid {
        return Err("No Realtek PCI Ethernet controller");
    }
    let Some(pci_dev) = pci::find_pci_device(desc.bus, desc.device, desc.function) else {
        return Err("Realtek NIC missing from PCI inventory");
    };
    let dev_id = device_add_pci_child(&pci_dev, "re");
    device_probe_and_attach(dev_id, &RE_NIC_DRIVER, mapping, allocator)
        .map_err(|e| e.as_str())?;
    Ok(stats().mac)
}

pub fn is_ready() -> bool {
    RE_READY_FAST.load(Ordering::Acquire)
}

pub fn stats() -> ReNicStats {
    let mut sc = RE_SOFTC.lock();
    let _ = sc.refresh_link();
    sc.snap
}

pub fn send_frame(frame: &[u8]) -> Result<(), &'static str> {
    RE_SOFTC.lock().send_frame(frame)
}

pub fn poll_frame(out: &mut [u8]) -> Option<usize> {
    RE_SOFTC.lock().poll_frame(out)
}
