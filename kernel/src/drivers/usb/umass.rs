use alloc::vec;
use core::ptr::copy_nonoverlapping;

use crate::freebsd_kpi::{bus_dmamap_sync, bus_dmamem_alloc, BUS_DMASYNC_POSTREAD, BUS_DMASYNC_PREWRITE, BUS_DMA_NOWAIT, BUS_DMA_ZERO};
use crate::freebsd_kpi::{BusDmaMap, BusDmaTag, Mtx};
use crate::memory::vmm::{PhysicalMemoryMapping, PAGE_SIZE};
use crate::memory::PhysicalFrameAllocator;
use crate::storage::block::{BlockDevice, BlockDeviceError, DEFAULT_SECTOR_SIZE};

use super::xhci;

/// USB Mass Storage Bulk-Only Transport (BOT) + SCSI Transparent Command Set.

const CBW_SIGNATURE: u32 = 0x4342_5355; // "USBC"
const CSW_SIGNATURE: u32 = 0x5342_5355; // "USBS"

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct Cbw {
    sig: u32,
    tag: u32,
    data_len: u32,
    flags: u8,
    lun: u8,
    cb_len: u8,
    cb: [u8; 16],
}

#[derive(Clone, Copy)]
pub struct UsbMassSnapshot {
    pub present: bool,
    pub ready: bool,
    pub slot_id: u8,
    pub bulk_in_dci: u8,
    pub bulk_out_dci: u8,
    pub block_size: u32,
    pub sector_count: u64,
    pub vendor: [u8; 8],
    pub product: [u8; 16],
    pub reads: u64,
    pub writes: u64,
    pub max_dma_sectors: usize,
}

impl UsbMassSnapshot {
    pub const EMPTY: Self = Self {
        present: false,
        ready: false,
        slot_id: 0,
        bulk_in_dci: 0,
        bulk_out_dci: 0,
        block_size: 512,
        sector_count: 0,
        vendor: [b' '; 8],
        product: [b' '; 16],
        reads: 0,
        writes: 0,
        max_dma_sectors: 8,
    };

    pub fn vendor_str(&self) -> &str {
        core::str::from_utf8(&self.vendor).unwrap_or("USB").trim()
    }

    pub fn product_str(&self) -> &str {
        core::str::from_utf8(&self.product).unwrap_or("Disk").trim()
    }
}

struct UmassState {
    snap: UsbMassSnapshot,
    tag: BusDmaTag,
    dma_map: BusDmaMap,
    next_tag: u32,
}

impl UmassState {
    const EMPTY: Self = Self {
        snap: UsbMassSnapshot::EMPTY,
        tag: BusDmaTag::EMPTY,
        dma_map: BusDmaMap::empty(),
        next_tag: 1,
    };

    fn scsi_cmd(
        &mut self,
        cdb: &[u8],
        data_in: bool,
        data_len: usize,
        write_payload: Option<&[u8]>,
    ) -> Result<usize, BlockDeviceError> {
        if !self.snap.present || self.dma_map.phys_addr() == 0 {
            return Err(BlockDeviceError::DeviceNotReady);
        }
        if data_len > PAGE_SIZE as usize {
            return Err(BlockDeviceError::InvalidBufferSize);
        }

        let tag = self.next_tag;
        self.next_tag = self.next_tag.wrapping_add(1).max(1);

        let mut cb = [0u8; 16];
        let clen = cdb.len().min(16);
        cb[..clen].copy_from_slice(&cdb[..clen]);

        let cbw = Cbw {
            sig: CBW_SIGNATURE,
            tag,
            data_len: data_len as u32,
            flags: if data_in { 0x80 } else { 0x00 },
            lun: 0,
            cb_len: clen as u8,
            cb,
        };

        // Page layout: [0..64) CBW, [64..128) CSW, [512..4096) Data staging if <= 3584, or reuse full page
        // Actually use separate offsets: [0..512) CBW/CSW, and when data_len > 0 use [512..4096) if <=3584 else fail;
        // Wait: 8 sectors = 4096 bytes! Let's allocate 2 pages (8192 bytes): page 0 = data [0..4096), page 1 = CBW/CSW [4096..4608)!
        // Because PAGE_SIZE = 4096 and we allocate 8192 bytes, page 0 [0..4096) is physically contiguous for data!
        let base_phys = self.dma_map.phys_addr();
        let base_virt = self.dma_map.virt_addr();
        let cbw_off = PAGE_SIZE as usize;
        let cbw_phys = self.dma_map.phys_addrs.get(1).copied().unwrap_or(base_phys + PAGE_SIZE);
        let cbw_virt = self.dma_map.virt_addrs.get(1).copied().unwrap_or(base_virt + PAGE_SIZE);

        unsafe {
            copy_nonoverlapping(&cbw as *const Cbw as *const u8, cbw_virt as *mut u8, 31);
        }
        bus_dmamap_sync(BUS_DMASYNC_PREWRITE);

        // 1. Send CBW on Bulk OUT
        if !xhci::bulk_transfer(self.snap.slot_id, self.snap.bulk_out_dci, false, cbw_phys, 31) {
            return Err(BlockDeviceError::IoError);
        }

        // 2. Data stage (if any)
        if data_len > 0 {
            if !data_in {
                if let Some(src) = write_payload {
                    let n = src.len().min(data_len);
                    unsafe {
                        copy_nonoverlapping(src.as_ptr(), base_virt as *mut u8, n);
                    }
                    bus_dmamap_sync(BUS_DMASYNC_PREWRITE);
                }
            }
            let dci = if data_in {
                self.snap.bulk_in_dci
            } else {
                self.snap.bulk_out_dci
            };
            if !xhci::bulk_transfer(self.snap.slot_id, dci, data_in, base_phys, data_len as u32) {
                return Err(BlockDeviceError::IoError);
            }
            if data_in {
                bus_dmamap_sync(BUS_DMASYNC_POSTREAD);
            }
        }

        // 3. Receive CSW (13 bytes) on Bulk IN
        unsafe { core::ptr::write_bytes(cbw_virt as *mut u8, 0, 16) };
        if !xhci::bulk_transfer(self.snap.slot_id, self.snap.bulk_in_dci, true, cbw_phys, 13) {
            return Err(BlockDeviceError::IoError);
        }
        bus_dmamap_sync(BUS_DMASYNC_POSTREAD);

        let mut csw = [0u8; 13];
        unsafe {
            copy_nonoverlapping(cbw_virt as *const u8, csw.as_mut_ptr(), 13);
        }
        let sig = u32::from_le_bytes([csw[0], csw[1], csw[2], csw[3]]);
        let csw_tag = u32::from_le_bytes([csw[4], csw[5], csw[6], csw[7]]);
        let status = csw[12];
        if sig != CSW_SIGNATURE || csw_tag != tag || status != 0 {
            return Err(BlockDeviceError::IoError);
        }
        Ok(data_len)
    }

    fn copy_data_out(&self, out: &mut [u8], len: usize) {
        let n = out.len().min(len).min(PAGE_SIZE as usize);
        unsafe {
            copy_nonoverlapping(self.dma_map.virt_addr() as *const u8, out.as_mut_ptr(), n);
        }
    }

    fn rw_sectors(
        &mut self,
        write: bool,
        start_lba: u64,
        sector_count: usize,
        buffer: &mut [u8],
    ) -> Result<(), BlockDeviceError> {
        if !self.snap.ready {
            return Err(BlockDeviceError::DeviceNotReady);
        }
        let needed = sector_count * 512;
        if sector_count == 0 || sector_count > self.snap.max_dma_sectors || buffer.len() < needed {
            return Err(BlockDeviceError::InvalidBufferSize);
        }
        if start_lba + sector_count as u64 > self.snap.sector_count {
            return Err(BlockDeviceError::SectorOutOfRange);
        }

        let op = if write { 0x2Au8 } else { 0x28u8 };
        let lba32 = (start_lba as u32).to_be_bytes();
        let cnt16 = (sector_count as u16).to_be_bytes();
        let cdb = [
            op, 0, lba32[0], lba32[1], lba32[2], lba32[3], 0, cnt16[0], cnt16[1], 0,
        ];

        if write {
            self.scsi_cmd(&cdb, false, needed, Some(&buffer[..needed]))?;
            self.snap.writes += sector_count as u64;
        } else {
            self.scsi_cmd(&cdb, true, needed, None)?;
            self.copy_data_out(buffer, needed);
            self.snap.reads += sector_count as u64;
        }
        Ok(())
    }
}

static UMASS: Mtx<UmassState> = Mtx::new("umass0", UmassState::EMPTY);

pub fn init_unit(
    slot_id: u8,
    bulk_in_dci: u8,
    bulk_out_dci: u8,
    dma_tag: BusDmaTag,
    mapping: &PhysicalMemoryMapping,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> bool {
    let Ok(dmap) = bus_dmamem_alloc(
        &dma_tag,
        (PAGE_SIZE as usize) * 2,
        BUS_DMA_NOWAIT | BUS_DMA_ZERO,
        mapping,
        allocator,
    ) else {
        return false;
    };

    let mut st = UMASS.lock();
    st.snap = UsbMassSnapshot::EMPTY;
    st.snap.present = true;
    st.snap.slot_id = slot_id;
    st.snap.bulk_in_dci = bulk_in_dci;
    st.snap.bulk_out_dci = bulk_out_dci;
    st.tag = dma_tag;
    st.dma_map = dmap;
    st.next_tag = 1;

    // 1. SCSI INQUIRY (0x12)
    let mut inq = [0u8; 36];
    if st.scsi_cmd(&[0x12, 0, 0, 0, 36, 0], true, 36, None).is_ok() {
        st.copy_data_out(&mut inq, 36);
        st.snap.vendor.copy_from_slice(&inq[8..16]);
        st.snap.product.copy_from_slice(&inq[16..32]);
    }

    // 2. TEST UNIT READY (0x00)
    for _ in 0..4 {
        if st.scsi_cmd(&[0, 0, 0, 0, 0, 0], true, 0, None).is_ok() {
            break;
        }
        crate::interrupts::timer::sleep_ms(10);
    }

    // 3. READ CAPACITY(10) (0x25)
    let mut cap = [0u8; 8];
    if st.scsi_cmd(&[0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0], true, 8, None).is_ok() {
        st.copy_data_out(&mut cap, 8);
        let last_lba = u32::from_be_bytes([cap[0], cap[1], cap[2], cap[3]]) as u64;
        let blksz = u32::from_be_bytes([cap[4], cap[5], cap[6], cap[7]]);
        if last_lba > 0 && blksz >= 512 {
            st.snap.block_size = blksz;
            st.snap.sector_count = (last_lba + 1).saturating_mul((blksz / 512) as u64);
            st.snap.ready = true;
        }
    }

    st.snap.ready
}

pub fn snapshot() -> UsbMassSnapshot {
    UMASS.lock().snap
}

pub fn is_ready() -> bool {
    UMASS.lock().snap.ready
}

pub struct UsbMassBlockDevice;

impl BlockDevice for UsbMassBlockDevice {
    fn sector_size(&self) -> u32 {
        DEFAULT_SECTOR_SIZE
    }

    fn sector_count(&self) -> u64 {
        UMASS.lock().snap.sector_count
    }

    fn max_transfer_sectors(&self) -> usize {
        UMASS.lock().snap.max_dma_sectors.max(1)
    }

    fn read_sector(&mut self, lba: u64, buffer: &mut [u8]) -> Result<(), BlockDeviceError> {
        UMASS.lock().rw_sectors(false, lba, 1, buffer)
    }

    fn write_sector(&mut self, lba: u64, buffer: &[u8]) -> Result<(), BlockDeviceError> {
        if buffer.len() < 512 {
            return Err(BlockDeviceError::InvalidBufferSize);
        }
        let mut tmp = [0u8; 512];
        tmp.copy_from_slice(&buffer[..512]);
        UMASS.lock().rw_sectors(true, lba, 1, &mut tmp)
    }

    fn read_sectors(
        &mut self,
        start_lba: u64,
        sector_count: usize,
        buffer: &mut [u8],
    ) -> Result<(), BlockDeviceError> {
        UMASS.lock().rw_sectors(false, start_lba, sector_count, buffer)
    }

    fn write_sectors(
        &mut self,
        start_lba: u64,
        sector_count: usize,
        buffer: &[u8],
    ) -> Result<(), BlockDeviceError> {
        let needed = sector_count * 512;
        if buffer.len() < needed {
            return Err(BlockDeviceError::InvalidBufferSize);
        }
        let mut scratch = vec![0u8; needed];
        scratch.copy_from_slice(&buffer[..needed]);
        UMASS.lock().rw_sectors(true, start_lba, sector_count, &mut scratch)
    }

    fn flush(&mut self) -> Result<(), BlockDeviceError> {
        let _ = UMASS
            .lock()
            .scsi_cmd(&[0x35, 0, 0, 0, 0, 0, 0, 0, 0, 0], false, 0, None);
        Ok(())
    }
}
