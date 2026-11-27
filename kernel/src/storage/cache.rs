use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use crate::storage::block::{BlockDevice, BlockDeviceError, DEFAULT_SECTOR_SIZE};

pub const CACHE_PAGE_SECTORS: usize = 8;
pub const CACHE_PAGE_BYTES: usize = CACHE_PAGE_SECTORS * DEFAULT_SECTOR_SIZE as usize; // 4096 B
pub const DEFAULT_CACHE_MAX_PAGES: usize = 512; // 2 MiB page cache
const DIRTY_FLUSH_HIGH_WATERMARK: usize = 64; // 256 KiB write-back buffer
const MAX_READAHEAD_PAGES: usize = 4; // up to 16 KiB (32 sectors) sequential read-ahead

static STAT_HITS: AtomicU64 = AtomicU64::new(0);
static STAT_MISSES: AtomicU64 = AtomicU64::new(0);
static STAT_READAHEAD_PAGES: AtomicU64 = AtomicU64::new(0);
static STAT_EVICTIONS: AtomicU64 = AtomicU64::new(0);
static STAT_FLUSHES: AtomicU64 = AtomicU64::new(0);
static STAT_WRITEBACK_SECTORS: AtomicU64 = AtomicU64::new(0);
static STAT_DMA_MULTI_READS: AtomicU64 = AtomicU64::new(0);
static STAT_DMA_MULTI_WRITES: AtomicU64 = AtomicU64::new(0);
static STAT_CACHED_PAGES: AtomicUsize = AtomicUsize::new(0);
static STAT_MAX_PAGES: AtomicUsize = AtomicUsize::new(DEFAULT_CACHE_MAX_PAGES);
static STAT_DIRTY_PAGES: AtomicUsize = AtomicUsize::new(0);
static STAT_MAX_DMA_SECTORS: AtomicUsize = AtomicUsize::new(1);
static STAT_BACKEND_CODE: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CacheBackendKind {
    None = 0,
    VirtioBlk = 1,
    AhciSata = 2,
    IdeAtaPio = 3,
    Nvme = 4,
    UsbMass = 5,
    Sdhci = 6,
}

impl CacheBackendKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "Unmounted",
            Self::VirtioBlk => "VirtIO-Blk DMA",
            Self::AhciSata => "AHCI SATA DMA",
            Self::IdeAtaPio => "ATA PIO (Multi-sector)",
            Self::Nvme => "NVMe PCI DMA (FreeBSD KPI)",
            Self::UsbMass => "USB Mass Storage (xHCI BOT)",
            Self::Sdhci => "SDHCI SD/MMC Card (FreeBSD KPI)",
        }
    }

    fn from_code(code: usize) -> Self {
        match code {
            1 => Self::VirtioBlk,
            2 => Self::AhciSata,
            3 => Self::IdeAtaPio,
            4 => Self::Nvme,
            5 => Self::UsbMass,
            6 => Self::Sdhci,
            _ => Self::None,
        }
    }
}

#[derive(Clone, Copy)]
pub struct BlockCacheStats {
    pub backend: CacheBackendKind,
    pub max_dma_sectors: usize,
    pub cached_pages: usize,
    pub max_pages: usize,
    pub dirty_pages: usize,
    pub hits: u64,
    pub misses: u64,
    pub readahead_pages: u64,
    pub evictions: u64,
    pub flushes: u64,
    pub writeback_sectors: u64,
    pub dma_multi_reads: u64,
    pub dma_multi_writes: u64,
}

pub fn stats_snapshot() -> BlockCacheStats {
    BlockCacheStats {
        backend: CacheBackendKind::from_code(STAT_BACKEND_CODE.load(Ordering::Relaxed)),
        max_dma_sectors: STAT_MAX_DMA_SECTORS.load(Ordering::Relaxed),
        cached_pages: STAT_CACHED_PAGES.load(Ordering::Relaxed),
        max_pages: STAT_MAX_PAGES.load(Ordering::Relaxed),
        dirty_pages: STAT_DIRTY_PAGES.load(Ordering::Relaxed),
        hits: STAT_HITS.load(Ordering::Relaxed),
        misses: STAT_MISSES.load(Ordering::Relaxed),
        readahead_pages: STAT_READAHEAD_PAGES.load(Ordering::Relaxed),
        evictions: STAT_EVICTIONS.load(Ordering::Relaxed),
        flushes: STAT_FLUSHES.load(Ordering::Relaxed),
        writeback_sectors: STAT_WRITEBACK_SECTORS.load(Ordering::Relaxed),
        dma_multi_reads: STAT_DMA_MULTI_READS.load(Ordering::Relaxed),
        dma_multi_writes: STAT_DMA_MULTI_WRITES.load(Ordering::Relaxed),
    }
}

pub fn reset_global_stats(backend: CacheBackendKind, max_dma_sectors: usize, max_pages: usize) {
    STAT_BACKEND_CODE.store(backend as usize, Ordering::Relaxed);
    STAT_MAX_DMA_SECTORS.store(max_dma_sectors.max(1), Ordering::Relaxed);
    STAT_MAX_PAGES.store(max_pages, Ordering::Relaxed);
    STAT_CACHED_PAGES.store(0, Ordering::Relaxed);
    STAT_DIRTY_PAGES.store(0, Ordering::Relaxed);
}

struct CachePage {
    page_lba: u64,
    valid_sectors: usize,
    referenced: bool,
    dirty: bool,
    data: Box<[u8; CACHE_PAGE_BYTES]>,
}

/// 4 KiB aligned Block/Page Cache with Clock (Second-Chance) eviction,
/// sequential multi-block DMA read-ahead, and coalesced write-back flushing.
pub struct CachedBlockDevice<D: BlockDevice> {
    inner: D,
    backend_kind: CacheBackendKind,
    max_pages: usize,
    pages: Vec<CachePage>,
    page_map: BTreeMap<u64, usize>,
    clock_hand: usize,
    dirty_count: usize,
    last_access_page: Option<u64>,
    io_scratch: Vec<u8>,
    publish_stats: bool,
}

impl<D: BlockDevice> CachedBlockDevice<D> {
    pub fn new(inner: D, backend_kind: CacheBackendKind) -> Self {
        Self::with_capacity(inner, backend_kind, DEFAULT_CACHE_MAX_PAGES, false)
    }

    pub fn with_capacity(
        inner: D,
        backend_kind: CacheBackendKind,
        max_pages: usize,
        publish_stats: bool,
    ) -> Self {
        let clamped_pages = max_pages.max(16);
        let max_dma = inner.max_transfer_sectors().max(1);
        if publish_stats {
            reset_global_stats(backend_kind, max_dma, clamped_pages);
        }
        Self {
            inner,
            backend_kind,
            max_pages: clamped_pages,
            pages: Vec::new(),
            page_map: BTreeMap::new(),
            clock_hand: 0,
            dirty_count: 0,
            last_access_page: None,
            io_scratch: Vec::new(),
            publish_stats,
        }
    }

    /// Enables live global telemetry updates once this volume wins disk probing.
    pub fn activate_telemetry(&mut self) {
        self.publish_stats = true;
        STAT_BACKEND_CODE.store(self.backend_kind as usize, Ordering::Relaxed);
        STAT_MAX_DMA_SECTORS.store(self.inner.max_transfer_sectors().max(1), Ordering::Relaxed);
        STAT_MAX_PAGES.store(self.max_pages, Ordering::Relaxed);
        STAT_CACHED_PAGES.store(self.pages.len(), Ordering::Relaxed);
        STAT_DIRTY_PAGES.store(self.dirty_count, Ordering::Relaxed);
    }

    pub fn inner(&self) -> &D {
        &self.inner
    }

    pub fn inner_mut(&mut self) -> &mut D {
        &mut self.inner
    }

    pub fn backend_kind(&self) -> CacheBackendKind {
        self.backend_kind
    }

    #[inline]
    fn sync_counts(&self) {
        if self.publish_stats {
            STAT_CACHED_PAGES.store(self.pages.len(), Ordering::Relaxed);
            STAT_DIRTY_PAGES.store(self.dirty_count, Ordering::Relaxed);
        }
    }

    #[inline]
    fn max_batch_pages(&self) -> usize {
        let dma_secs = self.inner.max_transfer_sectors().max(1);
        (dma_secs / CACHE_PAGE_SECTORS).max(1).min(16)
    }

    #[inline]
    fn valid_sectors_for_page(&self, page_lba: u64) -> usize {
        let total = self.inner.sector_count();
        if total == 0 {
            CACHE_PAGE_SECTORS
        } else if page_lba >= total {
            0
        } else {
            ((total - page_lba) as usize).min(CACHE_PAGE_SECTORS)
        }
    }

    fn ensure_scratch(&mut self, bytes: usize) {
        if self.io_scratch.len() < bytes {
            self.io_scratch.resize(bytes, 0);
        }
    }

    /// Acquires a slot index for `new_page_lba`, evicting a Clock victim if full.
    /// Any page in `[protect_start .. protect_end)` is skipped from eviction.
    fn acquire_slot_for(
        &mut self,
        new_page_lba: u64,
        valid_sectors: usize,
        protect_start: u64,
        protect_end: u64,
    ) -> Result<usize, BlockDeviceError> {
        if let Some(&existing) = self.page_map.get(&new_page_lba) {
            return Ok(existing);
        }

        if self.pages.len() < self.max_pages {
            let idx = self.pages.len();
            self.pages.push(CachePage {
                page_lba: new_page_lba,
                valid_sectors,
                referenced: true,
                dirty: false,
                data: Box::new([0u8; CACHE_PAGE_BYTES]),
            });
            self.page_map.insert(new_page_lba, idx);
            self.sync_counts();
            return Ok(idx);
        }

        let len = self.pages.len();
        let mut victim = self.clock_hand % len;
        let max_scans = len * 2 + 4;
        for _ in 0..max_scans {
            let p_lba = self.pages[victim].page_lba;
            let is_protected = p_lba >= protect_start && p_lba < protect_end;
            if is_protected || self.pages[victim].referenced {
                if !is_protected {
                    self.pages[victim].referenced = false;
                }
                victim = (victim + 1) % len;
            } else {
                break;
            }
        }
        self.clock_hand = (victim + 1) % len;

        // If victim is dirty, write it back before repurposing
        if self.pages[victim].dirty {
            let v_lba = self.pages[victim].page_lba;
            let v_secs = self.pages[victim].valid_sectors;
            if v_secs > 0 {
                let bytes = v_secs * DEFAULT_SECTOR_SIZE as usize;
                self.inner.write_sectors(v_lba, v_secs, &self.pages[victim].data[..bytes])?;
                if self.publish_stats {
                    STAT_WRITEBACK_SECTORS.fetch_add(v_secs as u64, Ordering::Relaxed);
                }
            }
            self.pages[victim].dirty = false;
            self.dirty_count = self.dirty_count.saturating_sub(1);
        }

        let old_lba = self.pages[victim].page_lba;
        self.page_map.remove(&old_lba);
        self.pages[victim].page_lba = new_page_lba;
        self.pages[victim].valid_sectors = valid_sectors;
        self.pages[victim].referenced = true;
        self.pages[victim].dirty = false;
        self.page_map.insert(new_page_lba, victim);

        if self.publish_stats {
            STAT_EVICTIONS.fetch_add(1, Ordering::Relaxed);
        }
        self.sync_counts();
        Ok(victim)
    }

    /// Loads `start_page_lba` (and optionally extra contiguous uncached pages) in one multi-sector I/O call.
    fn load_pages_batch(
        &mut self,
        start_page_lba: u64,
        requested_min_pages: usize,
        allow_readahead: bool,
    ) -> Result<usize, BlockDeviceError> {
        if let Some(&slot) = self.page_map.get(&start_page_lba) {
            self.pages[slot].referenced = true;
            return Ok(slot);
        }

        let is_seq = allow_readahead
            && self.last_access_page == Some(start_page_lba.saturating_sub(CACHE_PAGE_SECTORS as u64));
        let target_pages = if is_seq {
            requested_min_pages.max(1 + MAX_READAHEAD_PAGES)
        } else {
            requested_min_pages.max(1)
        }
        .min(self.max_batch_pages());

        let total = self.inner.sector_count();
        let mut batch_pages = 0usize;
        let mut batch_sectors = 0usize;
        while batch_pages < target_pages {
            let p_lba = start_page_lba + (batch_pages * CACHE_PAGE_SECTORS) as u64;
            if total > 0 && p_lba >= total {
                break;
            }
            if batch_pages > 0 && self.page_map.contains_key(&p_lba) {
                break;
            }
            let valid_secs = self.valid_sectors_for_page(p_lba);
            if valid_secs == 0 {
                break;
            }
            batch_sectors += valid_secs;
            batch_pages += 1;
            if valid_secs < CACHE_PAGE_SECTORS {
                break;
            }
        }

        if batch_pages == 0 || batch_sectors == 0 {
            return Err(BlockDeviceError::SectorOutOfRange);
        }

        let protect_end = start_page_lba + (batch_pages * CACHE_PAGE_SECTORS) as u64;

        if batch_pages == 1 {
            let valid_secs = batch_sectors;
            let bytes = valid_secs * DEFAULT_SECTOR_SIZE as usize;
            let slot = self.acquire_slot_for(start_page_lba, valid_secs, start_page_lba, protect_end)?;
            if let Err(e) = self.inner.read_sectors(start_page_lba, valid_secs, &mut self.pages[slot].data[..bytes]) {
                self.page_map.remove(&start_page_lba);
                self.pages[slot].valid_sectors = 0;
                return Err(e);
            }
            if bytes < CACHE_PAGE_BYTES {
                self.pages[slot].data[bytes..].fill(0);
            }
            if self.publish_stats {
                STAT_MISSES.fetch_add(1, Ordering::Relaxed);
                STAT_DMA_MULTI_READS.fetch_add(1, Ordering::Relaxed);
            }
            self.sync_counts();
            return Ok(slot);
        }

        let total_bytes = batch_sectors * DEFAULT_SECTOR_SIZE as usize;
        self.ensure_scratch(total_bytes);
        self.inner
            .read_sectors(start_page_lba, batch_sectors, &mut self.io_scratch[..total_bytes])?;

        if self.publish_stats {
            STAT_MISSES.fetch_add(1, Ordering::Relaxed);
            STAT_DMA_MULTI_READS.fetch_add(1, Ordering::Relaxed);
            if batch_pages > requested_min_pages {
                STAT_READAHEAD_PAGES.fetch_add((batch_pages - requested_min_pages) as u64, Ordering::Relaxed);
            }
        }

        let mut first_slot = 0usize;
        let mut scratch_off = 0usize;
        for p in 0..batch_pages {
            let p_lba = start_page_lba + (p * CACHE_PAGE_SECTORS) as u64;
            let valid_secs = self.valid_sectors_for_page(p_lba);
            let p_bytes = valid_secs * DEFAULT_SECTOR_SIZE as usize;
            let slot = self.acquire_slot_for(p_lba, valid_secs, start_page_lba, protect_end)?;
            self.pages[slot].data[..p_bytes]
                .copy_from_slice(&self.io_scratch[scratch_off..scratch_off + p_bytes]);
            if p_bytes < CACHE_PAGE_BYTES {
                self.pages[slot].data[p_bytes..].fill(0);
            }
            if p == 0 {
                first_slot = slot;
            }
            scratch_off += p_bytes;
        }

        self.sync_counts();
        Ok(first_slot)
    }

    fn flush_dirty_pages(&mut self) -> Result<(), BlockDeviceError> {
        if self.dirty_count == 0 {
            return Ok(());
        }

        let mut dirty_list: Vec<(u64, usize)> = self
            .pages
            .iter()
            .enumerate()
            .filter_map(|(idx, p)| if p.dirty { Some((p.page_lba, idx)) } else { None })
            .collect();

        if dirty_list.is_empty() {
            self.dirty_count = 0;
            self.sync_counts();
            return Ok(());
        }

        dirty_list.sort_unstable_by_key(|&(lba, _)| lba);

        let max_dma_secs = self.inner.max_transfer_sectors().max(CACHE_PAGE_SECTORS);
        let mut i = 0usize;
        while i < dirty_list.len() {
            let run_start_lba = dirty_list[i].0;
            let mut run_end_idx = i + 1;
            let mut run_sectors = self.pages[dirty_list[i].1].valid_sectors;

            if self.pages[dirty_list[i].1].valid_sectors == CACHE_PAGE_SECTORS {
                while run_end_idx < dirty_list.len() {
                    let prev_lba = dirty_list[run_end_idx - 1].0;
                    let cur_lba = dirty_list[run_end_idx].0;
                    let cur_secs = self.pages[dirty_list[run_end_idx].1].valid_sectors;
                    if cur_lba == prev_lba + CACHE_PAGE_SECTORS as u64
                        && run_sectors + cur_secs <= max_dma_secs
                    {
                        run_sectors += cur_secs;
                        run_end_idx += 1;
                        if cur_secs < CACHE_PAGE_SECTORS {
                            break;
                        }
                    } else {
                        break;
                    }
                }
            }

            if run_sectors > 0 {
                let run_bytes = run_sectors * DEFAULT_SECTOR_SIZE as usize;
                if run_end_idx - i == 1 {
                    let slot = dirty_list[i].1;
                    self.inner
                        .write_sectors(run_start_lba, run_sectors, &self.pages[slot].data[..run_bytes])?;
                } else {
                    self.ensure_scratch(run_bytes);
                    let mut off = 0usize;
                    for &(_, slot) in &dirty_list[i..run_end_idx] {
                        let b = self.pages[slot].valid_sectors * DEFAULT_SECTOR_SIZE as usize;
                        self.io_scratch[off..off + b].copy_from_slice(&self.pages[slot].data[..b]);
                        off += b;
                    }
                    self.inner
                        .write_sectors(run_start_lba, run_sectors, &self.io_scratch[..run_bytes])?;
                }

                if self.publish_stats {
                    STAT_WRITEBACK_SECTORS.fetch_add(run_sectors as u64, Ordering::Relaxed);
                    STAT_DMA_MULTI_WRITES.fetch_add(1, Ordering::Relaxed);
                }
            }

            for &(_, slot) in &dirty_list[i..run_end_idx] {
                if self.pages[slot].dirty {
                    self.pages[slot].dirty = false;
                    self.dirty_count = self.dirty_count.saturating_sub(1);
                }
            }

            i = run_end_idx;
        }

        self.sync_counts();
        Ok(())
    }
}

impl<D: BlockDevice> BlockDevice for CachedBlockDevice<D> {
    fn sector_size(&self) -> u32 {
        self.inner.sector_size()
    }

    fn sector_count(&self) -> u64 {
        self.inner.sector_count()
    }

    fn max_transfer_sectors(&self) -> usize {
        self.inner.max_transfer_sectors()
    }

    fn read_sector(&mut self, lba: u64, buffer: &mut [u8]) -> Result<(), BlockDeviceError> {
        self.read_sectors(lba, 1, buffer)
    }

    fn write_sector(&mut self, lba: u64, buffer: &[u8]) -> Result<(), BlockDeviceError> {
        self.write_sectors(lba, 1, buffer)
    }

    fn read_sectors(
        &mut self,
        start_lba: u64,
        sector_count: usize,
        buffer: &mut [u8],
    ) -> Result<(), BlockDeviceError> {
        if self.inner.sector_size() != DEFAULT_SECTOR_SIZE {
            return self.inner.read_sectors(start_lba, sector_count, buffer);
        }
        if sector_count == 0 {
            return if buffer.is_empty() {
                Ok(())
            } else {
                Err(BlockDeviceError::InvalidBufferSize)
            };
        }
        let expected = sector_count
            .checked_mul(DEFAULT_SECTOR_SIZE as usize)
            .ok_or(BlockDeviceError::InvalidBufferSize)?;
        if buffer.len() != expected {
            return Err(BlockDeviceError::InvalidBufferSize);
        }
        let total = self.inner.sector_count();
        if total > 0 {
            let end = start_lba
                .checked_add(sector_count as u64)
                .ok_or(BlockDeviceError::SectorOutOfRange)?;
            if end > total {
                return Err(BlockDeviceError::SectorOutOfRange);
            }
        }

        let mut cur_lba = start_lba;
        let mut rem_sectors = sector_count;
        let mut buf_off = 0usize;

        while rem_sectors > 0 {
            let page_lba = (cur_lba / CACHE_PAGE_SECTORS as u64) * CACHE_PAGE_SECTORS as u64;
            let in_page_sec = (cur_lba - page_lba) as usize;
            let take_secs = (CACHE_PAGE_SECTORS - in_page_sec).min(rem_sectors);
            let take_bytes = take_secs * DEFAULT_SECTOR_SIZE as usize;
            let src_off = in_page_sec * DEFAULT_SECTOR_SIZE as usize;

            let slot = if let Some(&hit_slot) = self.page_map.get(&page_lba) {
                self.pages[hit_slot].referenced = true;
                if self.publish_stats {
                    STAT_HITS.fetch_add(take_secs as u64, Ordering::Relaxed);
                }
                hit_slot
            } else {
                let end_page_lba = ((cur_lba + rem_sectors as u64 - 1) / CACHE_PAGE_SECTORS as u64)
                    * CACHE_PAGE_SECTORS as u64;
                let needed_pages = ((end_page_lba - page_lba) / CACHE_PAGE_SECTORS as u64) as usize + 1;
                self.load_pages_batch(page_lba, needed_pages, true)?
            };

            buffer[buf_off..buf_off + take_bytes]
                .copy_from_slice(&self.pages[slot].data[src_off..src_off + take_bytes]);

            self.last_access_page = Some(page_lba);
            cur_lba += take_secs as u64;
            rem_sectors -= take_secs;
            buf_off += take_bytes;
        }

        Ok(())
    }

    fn write_sectors(
        &mut self,
        start_lba: u64,
        sector_count: usize,
        buffer: &[u8],
    ) -> Result<(), BlockDeviceError> {
        if self.inner.sector_size() != DEFAULT_SECTOR_SIZE {
            return self.inner.write_sectors(start_lba, sector_count, buffer);
        }
        if sector_count == 0 {
            return if buffer.is_empty() {
                Ok(())
            } else {
                Err(BlockDeviceError::InvalidBufferSize)
            };
        }
        let expected = sector_count
            .checked_mul(DEFAULT_SECTOR_SIZE as usize)
            .ok_or(BlockDeviceError::InvalidBufferSize)?;
        if buffer.len() != expected {
            return Err(BlockDeviceError::InvalidBufferSize);
        }
        let total = self.inner.sector_count();
        if total > 0 {
            let end = start_lba
                .checked_add(sector_count as u64)
                .ok_or(BlockDeviceError::SectorOutOfRange)?;
            if end > total {
                return Err(BlockDeviceError::SectorOutOfRange);
            }
        }

        let mut cur_lba = start_lba;
        let mut rem_sectors = sector_count;
        let mut buf_off = 0usize;

        while rem_sectors > 0 {
            let page_lba = (cur_lba / CACHE_PAGE_SECTORS as u64) * CACHE_PAGE_SECTORS as u64;
            let in_page_sec = (cur_lba - page_lba) as usize;
            let valid_secs = self.valid_sectors_for_page(page_lba);
            let take_secs = (valid_secs.saturating_sub(in_page_sec)).min(rem_sectors);
            if take_secs == 0 {
                return Err(BlockDeviceError::SectorOutOfRange);
            }
            let take_bytes = take_secs * DEFAULT_SECTOR_SIZE as usize;
            let dst_off = in_page_sec * DEFAULT_SECTOR_SIZE as usize;

            let slot = if let Some(&hit_slot) = self.page_map.get(&page_lba) {
                if self.publish_stats {
                    STAT_HITS.fetch_add(take_secs as u64, Ordering::Relaxed);
                }
                hit_slot
            } else if in_page_sec == 0 && take_secs == valid_secs {
                // Full-page overwrite: allocate slot without unnecessary disk read-before-write.
                let protect_end = page_lba + CACHE_PAGE_SECTORS as u64;
                let s = self.acquire_slot_for(page_lba, valid_secs, page_lba, protect_end)?;
                if take_bytes < CACHE_PAGE_BYTES {
                    self.pages[s].data[take_bytes..].fill(0);
                }
                if self.publish_stats {
                    STAT_HITS.fetch_add(take_secs as u64, Ordering::Relaxed);
                }
                s
            } else {
                // Partial uncached page write: read-modify-write via cache fill.
                self.load_pages_batch(page_lba, 1, false)?
            };

            self.pages[slot].data[dst_off..dst_off + take_bytes]
                .copy_from_slice(&buffer[buf_off..buf_off + take_bytes]);
            self.pages[slot].referenced = true;
            if !self.pages[slot].dirty {
                self.pages[slot].dirty = true;
                self.dirty_count += 1;
            }

            cur_lba += take_secs as u64;
            rem_sectors -= take_secs;
            buf_off += take_bytes;
        }

        self.sync_counts();

        if self.dirty_count >= DIRTY_FLUSH_HIGH_WATERMARK {
            self.flush_dirty_pages()?;
        }

        Ok(())
    }

    fn flush(&mut self) -> Result<(), BlockDeviceError> {
        self.flush_dirty_pages()?;
        if self.publish_stats {
            STAT_FLUSHES.fetch_add(1, Ordering::Relaxed);
        }
        self.inner.flush()
    }
}
