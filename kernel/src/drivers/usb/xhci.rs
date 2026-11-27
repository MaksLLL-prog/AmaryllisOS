use core::ptr::{copy_nonoverlapping, read_volatile, write_volatile};
use core::sync::atomic::{fence, AtomicBool, AtomicU64, Ordering};

use crate::freebsd_kpi::{
    bus_alloc_resource_any, bus_dma_tag_create, bus_dmamap_sync, bus_dmamem_alloc,
    bus_setup_intr, bus_space_barrier, bus_space_read_1, bus_space_read_2, bus_space_read_4,
    bus_space_write_4, bus_space_write_8, device_add_pci_child, device_probe_and_attach,
    pci_enable_busmaster, with_device_mut, BusDmaMap, BusDmaTag, BusSpaceHandle, DeviceId,
    DeviceT, DriverT, Mtx, BUS_DMASYNC_POSTREAD, BUS_DMASYNC_PREWRITE, BUS_DMA_NOWAIT,
    BUS_DMA_ZERO, BUS_PROBE_SPECIFIC, ENXIO, SYS_RES_MEMORY,
};
use crate::hardware::PciXhciDescriptor;
use crate::interrupts::timer;
use crate::memory::vmm::{PhysicalMemoryMapping, PAGE_SIZE};
use crate::memory::PhysicalFrameAllocator;
use crate::pci;

/// FreeBSD `xhci` (eXtensible Host Controller Interface, USB 3.x/2.x/1.x) FreeBSD-KPI driver.
/// Supports BIOS->OS legacy handoff, DCBAA + Scratchpads, Command/Event Rings,
/// port reset + Address Device + GET_DESCRIPTOR enumeration, HID Boot Keyboard/Mouse polling,
/// and USB Mass Storage (BOT) bulk transport.

const MAX_SLOTS: usize = 8;
const CMD_RING_TRBS: usize = 64;
const EVT_RING_TRBS: usize = 64;
const EP_RING_TRBS: usize = 64;

// Operational register offsets (relative to cap_len)
const XHCI_USBCMD: usize = 0x00;
const XHCI_USBSTS: usize = 0x04;
const XHCI_DNCTRL: usize = 0x14;
const XHCI_CRCR: usize = 0x18;
const XHCI_DCBAAP: usize = 0x30;
const XHCI_CONFIG: usize = 0x38;
const XHCI_PORTSC_BASE: usize = 0x400;

const USBCMD_RS: u32 = 1 << 0;
const USBCMD_HCRST: u32 = 1 << 1;
const USBCMD_INTE: u32 = 1 << 2;

const USBSTS_HCH: u32 = 1 << 0;
const USBSTS_EINT: u32 = 1 << 3;
const USBSTS_CNR: u32 = 1 << 11;

const PORTSC_CCS: u32 = 1 << 0;
const PORTSC_PED: u32 = 1 << 1;
const PORTSC_PR: u32 = 1 << 4;
const PORTSC_PLS_MASK: u32 = 0xF << 5;
const PORTSC_PP: u32 = 1 << 9;
const PORTSC_SPEED_SHIFT: u32 = 10;
const PORTSC_PRC: u32 = 1 << 21;
const PORTSC_CSC: u32 = 1 << 17;
const PORTSC_WRC: u32 = 1 << 19;
const PORTSC_CHANGE_BITS: u32 = (1 << 17) | (1 << 18) | (1 << 19) | (1 << 20) | (1 << 21) | (1 << 22);

// Interrupter 0 offsets (relative to rts_off + 0x20)
const XHCI_IMAN: usize = 0x00;
const XHCI_IMOD: usize = 0x04;
const XHCI_ERSTSZ: usize = 0x08;
const XHCI_ERSTBA: usize = 0x10;
const XHCI_ERDP: usize = 0x18;

// TRB types
const TRB_TYPE_NORMAL: u32 = 1;
const TRB_TYPE_SETUP: u32 = 2;
const TRB_TYPE_DATA: u32 = 3;
const TRB_TYPE_STATUS: u32 = 4;
const TRB_TYPE_LINK: u32 = 6;
const TRB_TYPE_ENABLE_SLOT: u32 = 9;
const TRB_TYPE_ADDRESS_DEVICE: u32 = 11;
const TRB_TYPE_CONFIGURE_EP: u32 = 12;
const TRB_TYPE_TRANSFER_EVENT: u32 = 32;
const TRB_TYPE_CMD_COMPLETION: u32 = 33;

const TRB_CC_SUCCESS: u32 = 1;
const TRB_CC_SHORT_PKT: u32 = 13;

#[repr(C, align(16))]
#[derive(Clone, Copy)]
struct Trb {
    param: u64,
    status: u32,
    ctrl: u32,
}

impl Trb {
    const ZERO: Self = Self {
        param: 0,
        status: 0,
        ctrl: 0,
    };
}

#[repr(C, align(16))]
#[derive(Clone, Copy)]
struct ErstEntry {
    addr: u64,
    size: u32,
    rsvd: u32,
}

#[derive(Clone, Copy)]
pub struct XhciSnapshot {
    pub present: bool,
    pub initialized: bool,
    pub mmio_phys: u64,
    pub mmio_virt: u64,
    pub hci_version: u16,
    pub max_slots: u8,
    pub max_ports: u8,
    pub irq_line: u8,
    pub irq_gsi: Option<u32>,
    pub irq_vector: Option<u8>,
    pub connected_ports: u8,
    pub enumerated_devices: u8,
    pub hid_keyboard_slot: u8,
    pub hid_mouse_slot: u8,
    pub msc_slot: u8,
    pub hid_reports_rx: u64,
    pub irq_count: u64,
}

impl XhciSnapshot {
    pub const EMPTY: Self = Self {
        present: false,
        initialized: false,
        mmio_phys: 0,
        mmio_virt: 0,
        hci_version: 0,
        max_slots: 0,
        max_ports: 0,
        irq_line: 0,
        irq_gsi: None,
        irq_vector: None,
        connected_ports: 0,
        enumerated_devices: 0,
        hid_keyboard_slot: 0,
        hid_mouse_slot: 0,
        msc_slot: 0,
        hid_reports_rx: 0,
        irq_count: 0,
    };
}

#[derive(Clone, Copy)]
struct RingCursor {
    phys: u64,
    virt: u64,
    enq: usize,
    cycle: bool,
    len: usize,
}

impl RingCursor {
    const EMPTY: Self = Self {
        phys: 0,
        virt: 0,
        enq: 0,
        cycle: true,
        len: EP_RING_TRBS,
    };

    fn init(&mut self, phys: u64, virt: u64, len: usize) {
        self.phys = phys;
        self.virt = virt;
        self.enq = 0;
        self.cycle = true;
        self.len = len;
        unsafe {
            core::ptr::write_bytes(virt as *mut u8, 0, len * 16);
            // Install Link TRB at len - 1 pointing back to ring head with Toggle Cycle (bit 1)
            let link = (virt as *mut Trb).add(len - 1);
            write_volatile(
                link,
                Trb {
                    param: phys,
                    status: 0,
                    ctrl: (TRB_TYPE_LINK << 10) | (1 << 1) | 1,
                },
            );
        }
    }

    fn push(&mut self, mut trb: Trb) -> u64 {
        let idx = self.enq;
        let ptr = unsafe { (self.virt as *mut Trb).add(idx) };
        let trb_phys = self.phys + (idx as u64) * 16;
        if self.cycle {
            trb.ctrl |= 1;
        } else {
            trb.ctrl &= !1;
        }
        unsafe { write_volatile(ptr, trb) };

        self.enq += 1;
        if self.enq >= self.len - 1 {
            // Update link TRB cycle bit to match current cycle, then toggle
            let link_ptr = unsafe { (self.virt as *mut Trb).add(self.len - 1) };
            let mut link_ctrl = (TRB_TYPE_LINK << 10) | (1 << 1);
            if self.cycle {
                link_ctrl |= 1;
            }
            unsafe {
                write_volatile(
                    link_ptr,
                    Trb {
                        param: self.phys,
                        status: 0,
                        ctrl: link_ctrl,
                    },
                );
            }
            self.enq = 0;
            self.cycle = !self.cycle;
        }
        trb_phys
    }
}

#[derive(Clone, Copy)]
struct EpChannel {
    active: bool,
    slot_id: u8,
    dci: u8,
    mps: u16,
    ring: RingCursor,
    buf_phys: u64,
    buf_virt: u64,
    armed_trb_phys: u64,
}

impl EpChannel {
    const EMPTY: Self = Self {
        active: false,
        slot_id: 0,
        dci: 0,
        mps: 8,
        ring: RingCursor::EMPTY,
        buf_phys: 0,
        buf_virt: 0,
        armed_trb_phys: 0,
    };
}

// DMA Layout across 16 pages:
// Page 0: DCBAA
// Page 1: Cmd Ring (+0) & ERST (+2048)
// Page 2: Event Ring (+0)
// Page 3: Input Context (+0) & EP0 Control Data Buffer (+2048)
// Pages 4..11: Slot 1..8 Device Context (+0) & EP0 Ring (+2048)
// Page 12: HID Keyboard Ring (+0) & Report Buffer (+2048)
// Page 13: HID Mouse Ring (+0) & Report Buffer (+2048)
// Page 14: Bulk IN Ring (+0)
// Page 15: Bulk OUT Ring (+0)
const XHCI_DMA_PAGES: usize = 16;

struct XhciSoftc {
    snap: XhciSnapshot,
    mmio: BusSpaceHandle,
    cap_len: usize,
    dboff: usize,
    rtsoff: usize,
    ctx_stride: usize,
    dma_tag: BusDmaTag,
    dma_map: BusDmaMap,
    scratchpad_map: BusDmaMap,
    cmd_ring: RingCursor,
    evt_deq: usize,
    evt_cycle: bool,
    ep0_rings: [RingCursor; MAX_SLOTS + 1],
    kbd: EpChannel,
    mouse: EpChannel,
    bulk_in: EpChannel,
    bulk_out: EpChannel,
    pending_kbd_report: Option<[u8; 8]>,
    pending_mouse_report: Option<[u8; 4]>,
}

impl XhciSoftc {
    const EMPTY: Self = Self {
        snap: XhciSnapshot::EMPTY,
        mmio: BusSpaceHandle::EMPTY,
        cap_len: 0,
        dboff: 0,
        rtsoff: 0,
        ctx_stride: 32,
        dma_tag: BusDmaTag::EMPTY,
        dma_map: BusDmaMap::empty(),
        scratchpad_map: BusDmaMap::empty(),
        cmd_ring: RingCursor::EMPTY,
        evt_deq: 0,
        evt_cycle: true,
        ep0_rings: [RingCursor::EMPTY; MAX_SLOTS + 1],
        kbd: EpChannel::EMPTY,
        mouse: EpChannel::EMPTY,
        bulk_in: EpChannel::EMPTY,
        bulk_out: EpChannel::EMPTY,
        pending_kbd_report: None,
        pending_mouse_report: None,
    };

    #[inline]
    fn cap_r4(&self, off: usize) -> u32 {
        bus_space_read_4(&self.mmio, off)
    }
    #[inline]
    fn op_r4(&self, off: usize) -> u32 {
        bus_space_read_4(&self.mmio, self.cap_len + off)
    }
    #[inline]
    fn op_w4(&self, off: usize, v: u32) {
        bus_space_write_4(&self.mmio, self.cap_len + off, v);
    }
    #[inline]
    fn op_w8(&self, off: usize, v: u64) {
        bus_space_write_8(&self.mmio, self.cap_len + off, v);
    }
    #[inline]
    fn rt_r4(&self, off: usize) -> u32 {
        bus_space_read_4(&self.mmio, self.rtsoff + 0x20 + off)
    }
    #[inline]
    fn rt_w4(&self, off: usize, v: u32) {
        bus_space_write_4(&self.mmio, self.rtsoff + 0x20 + off, v);
    }
    #[inline]
    fn rt_w8(&self, off: usize, v: u64) {
        bus_space_write_8(&self.mmio, self.rtsoff + 0x20 + off, v);
    }
    #[inline]
    fn ring_doorbell(&self, slot: u8, target: u8) {
        bus_space_barrier();
        bus_space_write_4(&self.mmio, self.dboff + (slot as usize) * 4, target as u32);
    }

    #[inline]
    fn page_phys(&self, p: usize) -> u64 {
        self.dma_map.phys_addrs.get(p).copied().unwrap_or(0)
    }
    #[inline]
    fn page_virt(&self, p: usize) -> u64 {
        self.dma_map.virt_addrs.get(p).copied().unwrap_or(0)
    }

    fn update_erdp(&self) {
        let er_phys = self.page_phys(2);
        let addr = er_phys + (self.evt_deq as u64) * 16;
        self.rt_w8(XHCI_ERDP, addr | (1 << 3)); // EHB = 1 clears Event Handler Busy
    }

    fn drain_events_once(&mut self, target_trb_phys: u64, want_cmd: bool) -> Option<(u32, u8, u32)> {
        let er_virt = self.page_virt(2);
        if er_virt == 0 {
            return None;
        }
        let mut matched = None;

        for _ in 0..EVT_RING_TRBS {
            let ev = unsafe { read_volatile((er_virt as *const Trb).add(self.evt_deq)) };
            let c = (ev.ctrl & 1) != 0;
            if c != self.evt_cycle {
                break;
            }
            let trb_type = (ev.ctrl >> 10) & 0x3F;
            let cc = (ev.status >> 24) & 0xFF;
            let residual = ev.status & 0xFF_FFFF;
            let slot_id = ((ev.ctrl >> 24) & 0xFF) as u8;

            // Advance event consumer pointer
            self.evt_deq += 1;
            if self.evt_deq >= EVT_RING_TRBS {
                self.evt_deq = 0;
                self.evt_cycle = !self.evt_cycle;
            }
            self.update_erdp();

            if trb_type == TRB_TYPE_TRANSFER_EVENT {
                // Dispatch asynchronous HID interrupt completions when matched!
                if self.kbd.active && ev.param == self.kbd.armed_trb_phys {
                    if cc == TRB_CC_SUCCESS || cc == TRB_CC_SHORT_PKT {
                        let mut rep = [0u8; 8];
                        unsafe {
                            copy_nonoverlapping(self.kbd.buf_virt as *const u8, rep.as_mut_ptr(), 8);
                        }
                        self.pending_kbd_report = Some(rep);
                        self.snap.hid_reports_rx += 1;
                    }
                    self.rearm_hid_kbd();
                } else if self.mouse.active && ev.param == self.mouse.armed_trb_phys {
                    if cc == TRB_CC_SUCCESS || cc == TRB_CC_SHORT_PKT {
                        let mut rep = [0u8; 4];
                        unsafe {
                            copy_nonoverlapping(self.mouse.buf_virt as *const u8, rep.as_mut_ptr(), 4);
                        }
                        self.pending_mouse_report = Some(rep);
                        self.snap.hid_reports_rx += 1;
                    }
                    self.rearm_hid_mouse();
                } else if !want_cmd && target_trb_phys != 0 && ev.param == target_trb_phys {
                    matched = Some((cc, slot_id, residual));
                }
            } else if want_cmd && trb_type == TRB_TYPE_CMD_COMPLETION && ev.param == target_trb_phys {
                matched = Some((cc, slot_id, residual));
            }
        }
        matched
    }

    fn exec_command(&mut self, trb: Trb) -> Option<(u32, u8)> {
        let trb_phys = self.cmd_ring.push(trb);
        bus_dmamap_sync(BUS_DMASYNC_PREWRITE);
        self.ring_doorbell(0, 0);

        for _ in 0..400 {
            bus_dmamap_sync(BUS_DMASYNC_POSTREAD);
            if let Some((cc, slot, _)) = self.drain_events_once(trb_phys, true) {
                return Some((cc, slot));
            }
            timer::sleep_ms(1);
        }
        None
    }

    fn wait_transfer_event(&mut self, trb_phys: u64, timeout_ms: usize) -> Option<(u32, u32)> {
        for _ in 0..timeout_ms {
            bus_dmamap_sync(BUS_DMASYNC_POSTREAD);
            if let Some((cc, _, residual)) = self.drain_events_once(trb_phys, false) {
                return Some((cc, residual));
            }
            timer::sleep_ms(1);
        }
        None
    }

    fn zero_input_context(&self) {
        let ic_virt = self.page_virt(3);
        unsafe {
            core::ptr::write_bytes(ic_virt as *mut u8, 0, 33 * self.ctx_stride);
        }
    }

    #[inline]
    unsafe fn ctx_write_u32(base: u64, ctx_index: usize, ctx_stride: usize, dw: usize, val: u32) {
        let ptr = (base + (ctx_index * ctx_stride + dw * 4) as u64) as *mut u32;
        write_volatile(ptr, val);
    }

    #[inline]
    unsafe fn ctx_write_u64(base: u64, ctx_index: usize, ctx_stride: usize, dw: usize, val: u64) {
        Self::ctx_write_u32(base, ctx_index, ctx_stride, dw, val as u32);
        Self::ctx_write_u32(base, ctx_index, ctx_stride, dw + 1, (val >> 32) as u32);
    }

    fn address_device(&mut self, slot_id: u8, port_num: u8, speed: u8) -> bool {
        let idx = slot_id as usize;
        if idx == 0 || idx > MAX_SLOTS {
            return false;
        }
        let dev_ctx_phys = self.page_phys(3 + idx);
        let dev_ctx_virt = self.page_virt(3 + idx);
        let ep0_ring_phys = dev_ctx_phys + 2048;
        let ep0_ring_virt = dev_ctx_virt + 2048;
        unsafe { core::ptr::write_bytes(dev_ctx_virt as *mut u8, 0, 2048) };
        self.ep0_rings[idx].init(ep0_ring_phys, ep0_ring_virt, EP_RING_TRBS);

        // Set DCBAA[slot_id] = dev_ctx_phys
        let dcbaa_virt = self.page_virt(0) as *mut u64;
        unsafe { write_volatile(dcbaa_virt.add(idx), dev_ctx_phys) };

        let mps: u16 = match speed {
            4 => 512, // SuperSpeed
            3 => 64,  // HighSpeed
            2 => 64,  // FullSpeed (or 8 before reading bMaxPacketSize0; 64 works on xHCI/QEMU)
            _ => 8,   // LowSpeed
        };

        self.zero_input_context();
        let ic_phys = self.page_phys(3);
        let ic_virt = self.page_virt(3);
        let stride = self.ctx_stride;

        unsafe {
            // Input Control Context (index 0): Add Slot Context (bit 0) + EP0 Context (bit 1)
            Self::ctx_write_u32(ic_virt, 0, stride, 1, 0x3);
            // Slot Context (index 1): Context Entries = 1 (bits 27..31), Speed (bits 20..23)
            Self::ctx_write_u32(
                ic_virt,
                1,
                stride,
                0,
                (1 << 27) | (((speed as u32) & 0xF) << 20),
            );
            // Root Hub Port Number (bits 16..23) in DW1
            Self::ctx_write_u32(ic_virt, 1, stride, 1, (port_num as u32) << 16);
            // EP0 Context (index 2): CErr=3 (bits 1..2), EP Type=4 Control (bits 3..5), MPS (bits 16..31)
            Self::ctx_write_u32(
                ic_virt,
                2,
                stride,
                1,
                (3 << 1) | (4 << 3) | ((mps as u32) << 16),
            );
            // TR Dequeue Pointer | DCS=1
            Self::ctx_write_u64(ic_virt, 2, stride, 2, ep0_ring_phys | 1);
            // Average TRB Length = 8
            Self::ctx_write_u32(ic_virt, 2, stride, 4, 8);
        }

        let cmd = Trb {
            param: ic_phys,
            status: 0,
            ctrl: (TRB_TYPE_ADDRESS_DEVICE << 10) | ((slot_id as u32) << 24),
        };
        matches!(self.exec_command(cmd), Some((TRB_CC_SUCCESS, _)))
    }

    fn control_transfer(
        &mut self,
        slot_id: u8,
        bm_request_type: u8,
        b_request: u8,
        w_value: u16,
        w_index: u16,
        data_in: bool,
        out_buf: Option<&mut [u8]>,
        w_length: u16,
    ) -> bool {
        let idx = slot_id as usize;
        if idx == 0 || idx > MAX_SLOTS {
            return false;
        }

        let setup_pkt: u64 = (bm_request_type as u64)
            | ((b_request as u64) << 8)
            | ((w_value as u64) << 16)
            | ((w_index as u64) << 32)
            | ((w_length as u64) << 48);

        let trt: u32 = if w_length == 0 {
            0
        } else if data_in {
            3
        } else {
            2
        };

        let data_phys = self.page_phys(3) + 2048;
        let data_virt = self.page_virt(3) + 2048;
        if w_length > 0 && data_in {
            unsafe { core::ptr::write_bytes(data_virt as *mut u8, 0, w_length.min(2048) as usize) };
        }

        // 1. Setup Stage TRB (IDT=1 bit 6)
        self.ep0_rings[idx].push(Trb {
            param: setup_pkt,
            status: 8,
            ctrl: (TRB_TYPE_SETUP << 10) | (1 << 6) | (trt << 16),
        });

        // 2. Data Stage TRB (optional)
        if w_length > 0 {
            let dir_bit = if data_in { 1u32 << 16 } else { 0 };
            self.ep0_rings[idx].push(Trb {
                param: data_phys,
                status: w_length.min(2048) as u32,
                ctrl: (TRB_TYPE_DATA << 10) | dir_bit,
            });
        }

        // 3. Status Stage TRB (IOC=1 bit 5; DIR=1 IN if no-data or OUT, DIR=0 OUT if IN)
        let status_dir = if w_length == 0 || !data_in { 1u32 << 16 } else { 0 };
        let status_phys = self.ep0_rings[idx].push(Trb {
            param: 0,
            status: 0,
            ctrl: (TRB_TYPE_STATUS << 10) | (1 << 5) | status_dir,
        });

        bus_dmamap_sync(BUS_DMASYNC_PREWRITE);
        self.ring_doorbell(slot_id, 1); // EP0 DCI = 1

        let Some((cc, _)) = self.wait_transfer_event(status_phys, 400) else {
            return false;
        };
        if cc != TRB_CC_SUCCESS && cc != TRB_CC_SHORT_PKT {
            return false;
        }
        if data_in && w_length > 0 {
            if let Some(dst) = out_buf {
                let n = dst.len().min(w_length as usize).min(2048);
                unsafe {
                    copy_nonoverlapping(data_virt as *const u8, dst.as_mut_ptr(), n);
                }
            }
        }
        true
    }

    fn configure_endpoint(
        &mut self,
        slot_id: u8,
        dci: u8,
        ep_type: u32,
        mps: u16,
        interval: u8,
        ring_phys: u64,
        max_ctx_entries: u32,
    ) -> bool {
        let ic_phys = self.page_phys(3);
        let ic_virt = self.page_virt(3);
        let dev_ctx_virt = self.page_virt(3 + slot_id as usize);
        let stride = self.ctx_stride;

        self.zero_input_context();
        unsafe {
            // Copy current Slot Context into Input Context Slot Context (index 1)
            copy_nonoverlapping(
                dev_ctx_virt as *const u8,
                (ic_virt + stride as u64) as *mut u8,
                stride,
            );
            // Update Add Flags: Slot Context (bit 0) | Target DCI bit
            Self::ctx_write_u32(ic_virt, 0, stride, 1, 1u32 | (1u32 << (dci as u32)));
            // Update Context Entries in Slot Context DW0
            let slot_dw0 = read_volatile((ic_virt + stride as u64) as *const u32);
            let new_dw0 = (slot_dw0 & !(0x1F << 27)) | ((max_ctx_entries & 0x1F) << 27);
            Self::ctx_write_u32(ic_virt, 1, stride, 0, new_dw0);

            // Configure Endpoint Context at index (dci + 1)
            let ep_ctx_idx = (dci as usize) + 1;
            let xhci_interval = if interval == 0 { 3 } else { interval.min(10) as u32 };
            Self::ctx_write_u32(ic_virt, ep_ctx_idx, stride, 0, xhci_interval << 16);
            Self::ctx_write_u32(
                ic_virt,
                ep_ctx_idx,
                stride,
                1,
                (3 << 1) | ((ep_type & 0x7) << 3) | ((mps as u32) << 16),
            );
            Self::ctx_write_u64(ic_virt, ep_ctx_idx, stride, 2, ring_phys | 1);
            let avg_trb = if ep_type == 7 || ep_type == 3 { 8 } else { 512 };
            Self::ctx_write_u32(
                ic_virt,
                ep_ctx_idx,
                stride,
                4,
                avg_trb | ((mps as u32) << 16),
            );
        }

        let cmd = Trb {
            param: ic_phys,
            status: 0,
            ctrl: (TRB_TYPE_CONFIGURE_EP << 10) | ((slot_id as u32) << 24),
        };
        matches!(self.exec_command(cmd), Some((TRB_CC_SUCCESS, _)))
    }

    fn rearm_hid_kbd(&mut self) {
        if !self.kbd.active {
            return;
        }
        let len = (self.kbd.mps as u32).clamp(8, 64);
        let trb_phys = self.kbd.ring.push(Trb {
            param: self.kbd.buf_phys,
            status: len,
            ctrl: (TRB_TYPE_NORMAL << 10) | (1 << 5) | (1 << 2), // IOC | ISP
        });
        self.kbd.armed_trb_phys = trb_phys;
        bus_dmamap_sync(BUS_DMASYNC_PREWRITE);
        self.ring_doorbell(self.kbd.slot_id, self.kbd.dci);
    }

    fn rearm_hid_mouse(&mut self) {
        if !self.mouse.active {
            return;
        }
        let len = (self.mouse.mps as u32).clamp(3, 64);
        let trb_phys = self.mouse.ring.push(Trb {
            param: self.mouse.buf_phys,
            status: len,
            ctrl: (TRB_TYPE_NORMAL << 10) | (1 << 5) | (1 << 2), // IOC | ISP
        });
        self.mouse.armed_trb_phys = trb_phys;
        bus_dmamap_sync(BUS_DMASYNC_PREWRITE);
        self.ring_doorbell(self.mouse.slot_id, self.mouse.dci);
    }
}

static XHCI_SOFTC: Mtx<XhciSoftc> = Mtx::new("xhci0", XhciSoftc::EMPTY);
static XHCI_READY: AtomicBool = AtomicBool::new(false);
static XHCI_IRQ_COUNT: AtomicU64 = AtomicU64::new(0);
static HID_POLL_GUARD: AtomicBool = AtomicBool::new(false);

fn ep_addr_to_dci(ep_addr: u8) -> u8 {
    let ep_num = ep_addr & 0x0F;
    let dir_in = (ep_addr & 0x80) != 0;
    if ep_num == 0 {
        1
    } else {
        ep_num * 2 + if dir_in { 1 } else { 0 }
    }
}

fn bios_handoff(sc: &XhciSoftc) {
    let hcc1 = sc.cap_r4(0x10);
    let mut off = (((hcc1 >> 16) & 0xFFFF) as usize) * 4;
    let mut hops = 0;
    while off != 0 && hops < 32 {
        hops += 1;
        let cap = sc.cap_r4(off);
        let id = (cap & 0xFF) as u8;
        if id == 1 {
            // USB Legacy Support Capability: set OS Owned (bit 24)
            bus_space_write_4(&sc.mmio, off, cap | (1 << 24));
            for _ in 0..50 {
                let v = sc.cap_r4(off);
                if (v & (1 << 16)) == 0 {
                    break;
                }
                timer::sleep_ms(1);
            }
            // Clear BIOS SMI enables in USBLEGCTLSTS (+4)
            let ctl = sc.cap_r4(off + 4);
            bus_space_write_4(&sc.mmio, off + 4, ctl & 0xE000_0000);
            break;
        }
        let next = ((cap >> 8) & 0xFF) as usize;
        if next == 0 {
            break;
        }
        off += next * 4;
    }
}

fn xhci_probe(dev: &DeviceT) -> i32 {
    if dev.class_code == 0x0C && dev.subclass == 0x03 && dev.progif == 0x30 {
        BUS_PROBE_SPECIFIC
    } else {
        ENXIO
    }
}

fn xhci_attach(
    dev_id: DeviceId,
    mapping: Option<&PhysicalMemoryMapping>,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> i32 {
    let Some(map) = mapping else {
        return ENXIO;
    };
    let Some(dev_snap) = with_device_mut(dev_id, |d| {
        pci_enable_busmaster(d, false, true);
        *d
    }) else {
        return ENXIO;
    };

    let Ok(mmio_res) = bus_alloc_resource_any(&dev_snap, SYS_RES_MEMORY, 0, mapping, allocator)
    else {
        return ENXIO;
    };
    let Some(mmio) = mmio_res.bus_handle else {
        return ENXIO;
    };

    let dma_tag = bus_dma_tag_create(
        "xhci_dma",
        64,
        u64::MAX,
        0xFFFF_FFFF_FFFF_FFFF,
        XHCI_DMA_PAGES * PAGE_SIZE as usize,
        XHCI_DMA_PAGES,
        PAGE_SIZE as usize,
    );
    let Ok(dma_map) = bus_dmamem_alloc(
        &dma_tag,
        XHCI_DMA_PAGES * PAGE_SIZE as usize,
        BUS_DMA_NOWAIT | BUS_DMA_ZERO,
        map,
        allocator,
    ) else {
        return ENXIO;
    };

    let irq_line = (crate::freebsd_kpi::pci_read_config(&dev_snap, 0x3C, 4) & 0xFF) as u8;
    let irq_route = if irq_line != 0 && irq_line != 0xFF {
        bus_setup_intr(irq_line, "xhci0", xhci_kpi_intr, dev_id).ok()
    } else {
        None
    };

    // Inspect controller capabilities before taking lock or doing scratchpad alloc
    let cap_len = bus_space_read_1(&mmio, 0x00) as usize;
    let hci_version = bus_space_read_2(&mmio, 0x02);
    let hcsparams1 = bus_space_read_4(&mmio, 0x04);
    let hcsparams2 = bus_space_read_4(&mmio, 0x08);
    let hccparams1 = bus_space_read_4(&mmio, 0x10);
    let dboff = (bus_space_read_4(&mmio, 0x14) & !0x3) as usize;
    let rtsoff = (bus_space_read_4(&mmio, 0x18) & !0x1F) as usize;

    let max_slots = ((hcsparams1 & 0xFF) as usize).min(MAX_SLOTS) as u8;
    let max_ports = ((hcsparams1 >> 24) & 0xFF) as u8;
    let csz = (hccparams1 & (1 << 2)) != 0;
    let ctx_stride = if csz { 64 } else { 32 };
    let max_scratchpad = ((((hcsparams2 >> 21) & 0x1F) << 5) | ((hcsparams2 >> 27) & 0x1F)) as usize;

    let scratchpad_map = if max_scratchpad > 0 && max_scratchpad <= 16 {
        let sp_tag = bus_dma_tag_create(
            "xhci_sp",
            PAGE_SIZE,
            u64::MAX,
            0xFFFF_FFFF_FFFF_FFFF,
            max_scratchpad * PAGE_SIZE as usize,
            max_scratchpad,
            PAGE_SIZE as usize,
        );
        bus_dmamem_alloc(
            &sp_tag,
            max_scratchpad * PAGE_SIZE as usize,
            BUS_DMA_NOWAIT | BUS_DMA_ZERO,
            map,
            allocator,
        )
        .unwrap_or_else(|_| BusDmaMap::empty())
    } else {
        BusDmaMap::empty()
    };

    let umass_dma_tag = bus_dma_tag_create(
        "umass_dma",
        PAGE_SIZE,
        u64::MAX,
        0xFFFF_FFFF_FFFF_FFFF,
        2 * PAGE_SIZE as usize,
        2,
        PAGE_SIZE as usize,
    );

    let mut msc_pending: Option<(u8, u8, u8)> = None;

    {
        let mut sc = XHCI_SOFTC.lock();
        *sc = XhciSoftc::EMPTY;
        sc.mmio = mmio;
        sc.cap_len = cap_len;
        sc.dboff = dboff;
        sc.rtsoff = rtsoff;
        sc.ctx_stride = ctx_stride;
        sc.dma_tag = dma_tag;
        sc.dma_map = dma_map;
        sc.scratchpad_map = scratchpad_map;
        sc.snap.present = true;
        sc.snap.mmio_phys = mmio_res.phys_start;
        sc.snap.mmio_virt = mmio.virt_base;
        sc.snap.hci_version = hci_version;
        sc.snap.max_slots = max_slots;
        sc.snap.max_ports = max_ports;
        sc.snap.irq_line = irq_line;
        sc.snap.irq_gsi = irq_route.map(|r| r.0);
        sc.snap.irq_vector = irq_route.map(|r| r.1);

        bios_handoff(&sc);

        // 1. Halt xHCI
        let cmd = sc.op_r4(XHCI_USBCMD);
        sc.op_w4(XHCI_USBCMD, cmd & !USBCMD_RS);
        for _ in 0..50 {
            if (sc.op_r4(XHCI_USBSTS) & USBSTS_HCH) != 0 {
                break;
            }
            timer::sleep_ms(1);
        }

        // 2. Reset xHCI (HCRST)
        sc.op_w4(XHCI_USBCMD, USBCMD_HCRST);
        for _ in 0..100 {
            let c = sc.op_r4(XHCI_USBCMD);
            let s = sc.op_r4(XHCI_USBSTS);
            if (c & USBCMD_HCRST) == 0 && (s & USBSTS_CNR) == 0 {
                break;
            }
            timer::sleep_ms(1);
        }

        // 3. Configure Max Device Slots Enabled
        sc.op_w4(XHCI_CONFIG, max_slots as u32);
        sc.op_w4(XHCI_DNCTRL, 0);

        // 4. Install Scratchpad Pointer Array into DCBAA[0] if requested (stored at Page 1 + 3072)
        let dcbaa_phys = sc.page_phys(0);
        let dcbaa_virt = sc.page_virt(0) as *mut u64;
        if !sc.scratchpad_map.phys_addrs.is_empty() {
            let sp_arr_virt = sc.page_virt(1) + 3072;
            let sp_arr_phys = sc.page_phys(1) + 3072;
            let ptrs = sp_arr_virt as *mut u64;
            for (i, &pa) in sc.scratchpad_map.phys_addrs.iter().enumerate() {
                unsafe { write_volatile(ptrs.add(i), pa) };
            }
            unsafe { write_volatile(dcbaa_virt, sp_arr_phys) };
        }
        sc.op_w8(XHCI_DCBAAP, dcbaa_phys);

        // 5. Initialize Command Ring (Page 1, offset 0)
        let cmd_phys = sc.page_phys(1);
        let cmd_virt = sc.page_virt(1);
        sc.cmd_ring.init(cmd_phys, cmd_virt, CMD_RING_TRBS);
        sc.op_w8(XHCI_CRCR, cmd_phys | 1); // RCS = 1

        // 6. Initialize Event Ring (Page 2) & ERST (Page 1, offset 2048)
        let er_phys = sc.page_phys(2);
        let erst_phys = sc.page_phys(1) + 2048;
        let erst_virt = (sc.page_virt(1) + 2048) as *mut ErstEntry;
        unsafe {
            write_volatile(
                erst_virt,
                ErstEntry {
                    addr: er_phys,
                    size: EVT_RING_TRBS as u32,
                    rsvd: 0,
                },
            );
        }
        sc.evt_deq = 0;
        sc.evt_cycle = true;
        sc.rt_w4(XHCI_ERSTSZ, 1);
        sc.rt_w8(XHCI_ERDP, er_phys);
        sc.rt_w8(XHCI_ERSTBA, erst_phys);
        sc.rt_w4(XHCI_IMOD, 500);
        sc.rt_w4(XHCI_IMAN, 0x3); // IP clear + IE enable

        // 7. Start controller (RS | INTE)
        let sts = sc.op_r4(XHCI_USBSTS);
        sc.op_w4(XHCI_USBSTS, sts);
        sc.op_w4(XHCI_USBCMD, USBCMD_RS | USBCMD_INTE);
        timer::sleep_ms(10);

        sc.snap.initialized = (sc.op_r4(XHCI_USBSTS) & USBSTS_HCH) == 0;
        if !sc.snap.initialized {
            return ENXIO;
        }

        // 8. Scan root hub ports, reset connected ports, enumerate USB devices
        let mut connected = 0u8;
        let mut enumerated = 0u8;

        let ports = max_ports.min(16);
        for port_idx in 0..ports {
            let port_off = XHCI_PORTSC_BASE + (port_idx as usize) * 0x10;
            let mut portsc = sc.op_r4(port_off);
            if (portsc & PORTSC_PP) == 0 {
                sc.op_w4(port_off, (portsc & !PORTSC_CHANGE_BITS) | PORTSC_PP);
                timer::sleep_ms(5);
                portsc = sc.op_r4(port_off);
            }
            if (portsc & PORTSC_CCS) == 0 {
                continue;
            }
            connected += 1;

            // If not enabled yet (USB2 needs Port Reset; USB3 auto-enables)
            if (portsc & PORTSC_PED) == 0 {
                sc.op_w4(port_off, (portsc & !PORTSC_CHANGE_BITS) | PORTSC_PR);
                for _ in 0..60 {
                    let cur = sc.op_r4(port_off);
                    if (cur & PORTSC_PRC) != 0 || (cur & PORTSC_PED) != 0 {
                        sc.op_w4(port_off, (cur & !PORTSC_CHANGE_BITS) | PORTSC_PRC | PORTSC_CSC | PORTSC_WRC);
                        break;
                    }
                    timer::sleep_ms(1);
                }
            }
            portsc = sc.op_r4(port_off);
            if (portsc & PORTSC_PED) == 0 {
                continue;
            }
            let speed = ((portsc >> PORTSC_SPEED_SHIFT) & 0xF) as u8;

            // Enable Slot
            let Some((TRB_CC_SUCCESS, slot_id)) = sc.exec_command(Trb {
                param: 0,
                status: 0,
                ctrl: TRB_TYPE_ENABLE_SLOT << 10,
            }) else {
                continue;
            };
            if slot_id == 0 || (slot_id as usize) > MAX_SLOTS {
                continue;
            }

            if !sc.address_device(slot_id, port_idx + 1, speed) {
                continue;
            }
            enumerated += 1;

            // Read 18-byte Device Descriptor
            let mut dev_desc = [0u8; 18];
            let _ = sc.control_transfer(
                slot_id,
                0x80,
                6, // GET_DESCRIPTOR
                0x0100,
                0,
                true,
                Some(&mut dev_desc),
                18,
            );

            // Read Config Descriptor (first 9 bytes to discover wTotalLength)
            let mut cfg_hdr = [0u8; 9];
            if !sc.control_transfer(slot_id, 0x80, 6, 0x0200, 0, true, Some(&mut cfg_hdr), 9) {
                continue;
            }
            let total_len = u16::from_le_bytes([cfg_hdr[2], cfg_hdr[3]]).min(256);
            let cfg_val = cfg_hdr[5].max(1);
            let mut cfg_buf = [0u8; 256];
            if !sc.control_transfer(
                slot_id,
                0x80,
                6,
                0x0200,
                0,
                true,
                Some(&mut cfg_buf[..total_len as usize]),
                total_len,
            ) {
                continue;
            }

            // SET_CONFIGURATION(cfg_val)
            let _ = sc.control_transfer(slot_id, 0x00, 9, cfg_val as u16, 0, false, None, 0);

            // Parse interfaces & endpoints in Configuration Descriptor
            let mut pos = 0usize;
            let mut cur_class = 0u8;
            let mut cur_sub = 0u8;
            let mut cur_proto = 0u8;
            let mut cur_iface = 0u8;

            let mut kbd_ep: Option<(u8, u16, u8)> = None;
            let mut mouse_ep: Option<(u8, u16, u8)> = None;
            let mut msc_in_ep: Option<(u8, u16)> = None;
            let mut msc_out_ep: Option<(u8, u16)> = None;

            while pos + 2 <= total_len as usize {
                let dlen = cfg_buf[pos] as usize;
                let dtype = cfg_buf[pos + 1];
                if dlen < 2 || pos + dlen > total_len as usize {
                    break;
                }
                if dtype == 4 && dlen >= 9 {
                    // Interface Descriptor
                    cur_iface = cfg_buf[pos + 2];
                    cur_class = cfg_buf[pos + 5];
                    cur_sub = cfg_buf[pos + 6];
                    cur_proto = cfg_buf[pos + 7];
                } else if dtype == 5 && dlen >= 7 {
                    // Endpoint Descriptor
                    let ep_addr = cfg_buf[pos + 2];
                    let bm_attr = cfg_buf[pos + 3] & 0x03;
                    let mps = u16::from_le_bytes([cfg_buf[pos + 4], cfg_buf[pos + 5]]) & 0x07FF;
                    let interval = cfg_buf[pos + 6];
                    let dir_in = (ep_addr & 0x80) != 0;

                    if cur_class == 0x03 && cur_sub == 0x01 && bm_attr == 3 && dir_in {
                        // HID Boot Keyboard (proto=1) or Mouse (proto=2)
                        if cur_proto == 1 && kbd_ep.is_none() {
                            kbd_ep = Some((ep_addr, mps.max(8), interval));
                            // SET_PROTOCOL(Boot = 0)
                            let _ = sc.control_transfer(
                                slot_id,
                                0x21,
                                0x0B,
                                0,
                                cur_iface as u16,
                                false,
                                None,
                                0,
                            );
                        } else if cur_proto == 2 && mouse_ep.is_none() {
                            mouse_ep = Some((ep_addr, mps.max(4), interval));
                            let _ = sc.control_transfer(
                                slot_id,
                                0x21,
                                0x0B,
                                0,
                                cur_iface as u16,
                                false,
                                None,
                                0,
                            );
                        }
                    } else if cur_class == 0x08 && cur_sub == 0x06 && cur_proto == 0x50 && bm_attr == 2
                    {
                        // USB Mass Storage Bulk-Only Transport (SCSI Transparent)
                        if dir_in && msc_in_ep.is_none() {
                            msc_in_ep = Some((ep_addr, mps.max(64)));
                        } else if !dir_in && msc_out_ep.is_none() {
                            msc_out_ep = Some((ep_addr, mps.max(64)));
                        }
                    }
                }
                pos += dlen;
            }

            // Configure HID Boot Keyboard Interrupt IN endpoint (Page 12)
            if let Some((ep_addr, mps, interval)) = kbd_ep {
                if !sc.kbd.active {
                    let dci = ep_addr_to_dci(ep_addr);
                    let r_phys = sc.page_phys(12);
                    let r_virt = sc.page_virt(12);
                    sc.kbd.ring.init(r_phys, r_virt, EP_RING_TRBS);
                    if sc.configure_endpoint(slot_id, dci, 7, mps, interval, r_phys, dci as u32) {
                        sc.kbd.active = true;
                        sc.kbd.slot_id = slot_id;
                        sc.kbd.dci = dci;
                        sc.kbd.mps = mps;
                        sc.kbd.buf_phys = r_phys + 2048;
                        sc.kbd.buf_virt = r_virt + 2048;
                        sc.snap.hid_keyboard_slot = slot_id;
                        sc.rearm_hid_kbd();
                    }
                }
            }

            // Configure HID Boot Mouse Interrupt IN endpoint (Page 13)
            if let Some((ep_addr, mps, interval)) = mouse_ep {
                if !sc.mouse.active {
                    let dci = ep_addr_to_dci(ep_addr);
                    let r_phys = sc.page_phys(13);
                    let r_virt = sc.page_virt(13);
                    sc.mouse.ring.init(r_phys, r_virt, EP_RING_TRBS);
                    if sc.configure_endpoint(slot_id, dci, 7, mps, interval, r_phys, dci as u32) {
                        sc.mouse.active = true;
                        sc.mouse.slot_id = slot_id;
                        sc.mouse.dci = dci;
                        sc.mouse.mps = mps;
                        sc.mouse.buf_phys = r_phys + 2048;
                        sc.mouse.buf_virt = r_virt + 2048;
                        sc.snap.hid_mouse_slot = slot_id;
                        sc.rearm_hid_mouse();
                    }
                }
            }

            // Configure USB Mass Storage Bulk IN (Page 14) & Bulk OUT (Page 15)
            if let (Some((in_addr, in_mps)), Some((out_addr, out_mps))) = (msc_in_ep, msc_out_ep) {
                if !sc.bulk_in.active {
                    let in_dci = ep_addr_to_dci(in_addr);
                    let out_dci = ep_addr_to_dci(out_addr);
                    let max_dci = (in_dci.max(out_dci)) as u32;

                    let in_phys = sc.page_phys(14);
                    let in_virt = sc.page_virt(14);
                    let out_phys = sc.page_phys(15);
                    let out_virt = sc.page_virt(15);
                    sc.bulk_in.ring.init(in_phys, in_virt, EP_RING_TRBS);
                    sc.bulk_out.ring.init(out_phys, out_virt, EP_RING_TRBS);

                    // Configure Bulk OUT (ep_type=2) then Bulk IN (ep_type=6)
                    if sc.configure_endpoint(slot_id, out_dci, 2, out_mps, 0, out_phys, max_dci)
                        && sc.configure_endpoint(slot_id, in_dci, 6, in_mps, 0, in_phys, max_dci)
                    {
                        sc.bulk_out.active = true;
                        sc.bulk_out.slot_id = slot_id;
                        sc.bulk_out.dci = out_dci;
                        sc.bulk_out.mps = out_mps;

                        sc.bulk_in.active = true;
                        sc.bulk_in.slot_id = slot_id;
                        sc.bulk_in.dci = in_dci;
                        sc.bulk_in.mps = in_mps;

                        sc.snap.msc_slot = slot_id;
                        msc_pending = Some((slot_id, in_dci, out_dci));
                    }
                }
            }

            let _ = PORTSC_PLS_MASK;
        }

        sc.snap.connected_ports = connected;
        sc.snap.enumerated_devices = enumerated;
        XHCI_READY.store(true, Ordering::Release);
    } // <-- XHCI_SOFTC lock dropped BEFORE calling umass::init_unit!

    if let Some((slot_id, in_dci, out_dci)) = msc_pending {
        let _ = super::umass::init_unit(slot_id, in_dci, out_dci, umass_dma_tag, map, allocator);
    }

    with_device_mut(dev_id, |d| {
        d.irq_line = irq_line;
        d.irq_gsi = irq_route.map(|r| r.0);
        d.irq_vector = irq_route.map(|r| r.1);
        d.set_desc("USB xHCI Host Controller");
    });

    0
}

fn xhci_detach(_dev_id: DeviceId) -> i32 {
    0
}

pub static XHCI_DRIVER: DriverT = DriverT {
    name: "xhci",
    probe: xhci_probe,
    attach: xhci_attach,
    detach: xhci_detach,
};

fn xhci_kpi_intr(_arg: usize) -> bool {
    irq_top_half()
}

pub fn irq_top_half() -> bool {
    if !XHCI_READY.load(Ordering::Acquire) {
        return false;
    }
    let Some(sc) = XHCI_SOFTC.try_lock() else {
        return false;
    };
    if !sc.snap.initialized {
        return false;
    }
    let sts = sc.op_r4(XHCI_USBSTS);
    let iman = sc.rt_r4(XHCI_IMAN);
    if (sts & USBSTS_EINT) == 0 && (iman & 1) == 0 {
        return false;
    }
    sc.op_w4(XHCI_USBSTS, sts & USBSTS_EINT);
    sc.rt_w4(XHCI_IMAN, iman | 1); // Write 1 to IP bit clears interrupt pending
    XHCI_IRQ_COUNT.fetch_add(1, Ordering::Relaxed);
    true
}

pub fn poll_hid() -> usize {
    if !XHCI_READY.load(Ordering::Acquire) {
        return 0;
    }
    if HID_POLL_GUARD.swap(true, Ordering::AcqRel) {
        return 0;
    }

    let (kbd_rep, mouse_rep) = {
        let Some(mut sc) = XHCI_SOFTC.try_lock() else {
            HID_POLL_GUARD.store(false, Ordering::Release);
            return 0;
        };
        if !sc.snap.initialized {
            HID_POLL_GUARD.store(false, Ordering::Release);
            return 0;
        }
        bus_dmamap_sync(BUS_DMASYNC_POSTREAD);
        let _ = sc.drain_events_once(0, false);
        (sc.pending_kbd_report.take(), sc.pending_mouse_report.take())
    };

    let mut count = 0usize;
    if let Some(rep) = kbd_rep {
        super::hid::process_keyboard_report(&rep);
        count += 1;
    }
    if let Some(rep) = mouse_rep {
        super::hid::process_mouse_report(&rep);
        count += 1;
    }

    HID_POLL_GUARD.store(false, Ordering::Release);
    count
}

pub fn bulk_transfer(slot_id: u8, dci: u8, data_in: bool, phys: u64, len: u32) -> bool {
    if !XHCI_READY.load(Ordering::Acquire) || phys == 0 || len == 0 {
        return false;
    }
    let mut sc = XHCI_SOFTC.lock();
    if !sc.snap.initialized {
        return false;
    }
    let trb_phys = if data_in {
        if !sc.bulk_in.active || sc.bulk_in.slot_id != slot_id || sc.bulk_in.dci != dci {
            return false;
        }
        sc.bulk_in.ring.push(Trb {
            param: phys,
            status: len,
            ctrl: (TRB_TYPE_NORMAL << 10) | (1 << 5) | (1 << 2),
        })
    } else {
        if !sc.bulk_out.active || sc.bulk_out.slot_id != slot_id || sc.bulk_out.dci != dci {
            return false;
        }
        sc.bulk_out.ring.push(Trb {
            param: phys,
            status: len,
            ctrl: (TRB_TYPE_NORMAL << 10) | (1 << 5),
        })
    };

    fence(Ordering::SeqCst);
    bus_dmamap_sync(BUS_DMASYNC_PREWRITE);
    sc.ring_doorbell(slot_id, dci);

    if let Some((cc, _)) = sc.wait_transfer_event(trb_phys, 1000) {
        cc == TRB_CC_SUCCESS || cc == TRB_CC_SHORT_PKT
    } else {
        false
    }
}

pub fn init(
    desc: &PciXhciDescriptor,
    mapping: Option<&PhysicalMemoryMapping>,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> Result<XhciSnapshot, &'static str> {
    if !desc.valid {
        return Err("No xHCI PCI controller discovered");
    }
    let Some(pci_dev) = pci::find_pci_device(desc.bus, desc.device, desc.function) else {
        return Err("xHCI PCI config device missing");
    };
    let dev_id = device_add_pci_child(&pci_dev, "xhci");
    device_probe_and_attach(dev_id, &XHCI_DRIVER, mapping, allocator)
        .map_err(|_| "xHCI probe/attach failed")?;
    Ok(snapshot())
}

pub fn is_ready() -> bool {
    XHCI_READY.load(Ordering::Acquire)
}

pub fn snapshot() -> XhciSnapshot {
    let mut s = XHCI_SOFTC.lock().snap;
    s.irq_count = XHCI_IRQ_COUNT.load(Ordering::Relaxed);
    s
}
