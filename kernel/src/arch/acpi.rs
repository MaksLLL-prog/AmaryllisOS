use core::ptr::read_unaligned;
use spin::Mutex;
use x86_64::structures::paging::PageTableFlags;
use x86_64::VirtAddr;

use crate::memory::vmm::{PhysicalMemoryMapping, PAGE_SIZE};
use crate::memory::PhysicalFrameAllocator;

const ACPI_SCRATCH_VIRT_BASE: u64 = 0x0000_5320_0000_0000;

pub const MAX_ACPI_TABLES: usize = 32;
pub const MAX_MADT_CPUS: usize = 64;
pub const MAX_MADT_IOAPICS: usize = 8;
pub const MAX_MADT_ISOS: usize = 24;

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct RsdpV1 {
    signature: [u8; 8],
    checksum: u8,
    oem_id: [u8; 6],
    revision: u8,
    rsdt_address: u32,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct RsdpV2 {
    v1: RsdpV1,
    length: u32,
    xsdt_address: u64,
    extended_checksum: u8,
    reserved: [u8; 3],
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct SdtHeader {
    signature: [u8; 4],
    length: u32,
    revision: u8,
    checksum: u8,
    oem_id: [u8; 6],
    oem_table_id: [u8; 8],
    oem_revision: u32,
    creator_id: u32,
    creator_revision: u32,
}

#[derive(Clone, Copy)]
pub struct AcpiTableSummary {
    pub signature: [u8; 4],
    pub phys_addr: u64,
    pub length: u32,
    pub revision: u8,
    pub checksum_ok: bool,
}

impl AcpiTableSummary {
    pub const EMPTY: Self = Self {
        signature: [0; 4],
        phys_addr: 0,
        length: 0,
        revision: 0,
        checksum_ok: false,
    };

    pub fn sig_str(&self) -> &str {
        core::str::from_utf8(&self.signature).unwrap_or("????")
    }
}

#[derive(Clone, Copy)]
pub struct MadtCpuEntry {
    pub acpi_processor_id: u8,
    pub apic_id: u8,
    pub enabled: bool,
    pub online_capable: bool,
}

impl MadtCpuEntry {
    pub const EMPTY: Self = Self {
        acpi_processor_id: 0,
        apic_id: 0,
        enabled: false,
        online_capable: false,
    };
}

#[derive(Clone, Copy)]
pub struct MadtIoApicEntry {
    pub ioapic_id: u8,
    pub phys_addr: u32,
    pub gsi_base: u32,
}

impl MadtIoApicEntry {
    pub const EMPTY: Self = Self {
        ioapic_id: 0,
        phys_addr: 0,
        gsi_base: 0,
    };
}

#[derive(Clone, Copy)]
pub struct MadtIsoEntry {
    pub bus_source: u8,
    pub irq_source: u8,
    pub gsi: u32,
    pub flags: u16,
}

impl MadtIsoEntry {
    pub const EMPTY: Self = Self {
        bus_source: 0,
        irq_source: 0,
        gsi: 0,
        flags: 0,
    };
}

#[derive(Clone, Copy)]
pub struct AcpiPlatformInfo {
    pub discovered: bool,
    pub revision: u8,
    pub oem_id: [u8; 6],
    pub rsdp_phys: u64,
    pub root_sdt_phys: u64,
    pub using_xsdt: bool,
    pub tables: [AcpiTableSummary; MAX_ACPI_TABLES],
    pub table_count: usize,
    pub lapic_phys: u64,
    pub pcat_compat: bool,
    pub cpus: [MadtCpuEntry; MAX_MADT_CPUS],
    pub cpu_count: usize,
    pub enabled_cpu_count: usize,
    pub ioapics: [MadtIoApicEntry; MAX_MADT_IOAPICS],
    pub ioapic_count: usize,
    pub isos: [MadtIsoEntry; MAX_MADT_ISOS],
    pub iso_count: usize,
    pub fadt_sci_irq: u16,
    pub fadt_pm1a_cnt: u32,
    pub fadt_pm1b_cnt: u32,
    pub fadt_pm_tmr: u32,
    pub fadt_century_reg: u8,
    pub mcfg_ecam_base: u64,
    pub mcfg_start_bus: u8,
    pub mcfg_end_bus: u8,
    pub hpet_phys: u64,
}

impl AcpiPlatformInfo {
    pub const EMPTY: Self = Self {
        discovered: false,
        revision: 0,
        oem_id: [0; 6],
        rsdp_phys: 0,
        root_sdt_phys: 0,
        using_xsdt: false,
        tables: [AcpiTableSummary::EMPTY; MAX_ACPI_TABLES],
        table_count: 0,
        lapic_phys: 0,
        pcat_compat: false,
        cpus: [MadtCpuEntry::EMPTY; MAX_MADT_CPUS],
        cpu_count: 0,
        enabled_cpu_count: 0,
        ioapics: [MadtIoApicEntry::EMPTY; MAX_MADT_IOAPICS],
        ioapic_count: 0,
        isos: [MadtIsoEntry::EMPTY; MAX_MADT_ISOS],
        iso_count: 0,
        fadt_sci_irq: 0,
        fadt_pm1a_cnt: 0,
        fadt_pm1b_cnt: 0,
        fadt_pm_tmr: 0,
        fadt_century_reg: 0,
        mcfg_ecam_base: 0,
        mcfg_start_bus: 0,
        mcfg_end_bus: 0,
        hpet_phys: 0,
    };

    pub fn oem_str(&self) -> &str {
        core::str::from_utf8(&self.oem_id).unwrap_or("OEM").trim()
    }
}

static ACPI_INFO: Mutex<AcpiPlatformInfo> = Mutex::new(AcpiPlatformInfo::EMPTY);

struct AcpiReader<'m, 'a, 'r> {
    mapping: &'m PhysicalMemoryMapping,
    allocator: &'a mut PhysicalFrameAllocator<'r>,
    scratch_phys_page: Option<u64>,
}

impl<'m, 'a, 'r> AcpiReader<'m, 'a, 'r> {
    fn new(
        mapping: &'m PhysicalMemoryMapping,
        allocator: &'a mut PhysicalFrameAllocator<'r>,
    ) -> Self {
        Self {
            mapping,
            allocator,
            scratch_phys_page: None,
        }
    }

    fn map_or_get_page(&mut self, page_phys: u64) -> Option<u64> {
        if let Some(hhdm_va) = self.mapping.translate(page_phys) {
            if self.mapping.translate_virtual_address(hhdm_va).is_some() {
                return Some(hhdm_va.as_u64());
            }
        }

        let scratch_va = VirtAddr::new(ACPI_SCRATCH_VIRT_BASE);
        if self.scratch_phys_page == Some(page_phys) {
            return Some(ACPI_SCRATCH_VIRT_BASE);
        }

        if self.scratch_phys_page.is_some() {
            let _ = self.mapping.unmap_page(scratch_va);
            self.scratch_phys_page = None;
        } else if self.mapping.translate_virtual_address(scratch_va).is_some() {
            let _ = self.mapping.unmap_page(scratch_va);
        }

        let flags = PageTableFlags::PRESENT;
        if self
            .mapping
            .map_page(self.allocator, scratch_va, page_phys, flags)
            .is_ok()
        {
            self.scratch_phys_page = Some(page_phys);
            Some(ACPI_SCRATCH_VIRT_BASE)
        } else {
            None
        }
    }

    fn read_bytes(&mut self, phys: u64, out: &mut [u8]) -> bool {
        if out.is_empty() {
            return true;
        }
        let mut copied = 0usize;
        while copied < out.len() {
            let Some(cur_phys) = phys.checked_add(copied as u64) else {
                return false;
            };
            let page_phys = cur_phys & !(PAGE_SIZE - 1);
            let page_off = (cur_phys & (PAGE_SIZE - 1)) as usize;
            let chunk = (out.len() - copied).min(PAGE_SIZE as usize - page_off);

            let Some(page_virt) = self.map_or_get_page(page_phys) else {
                return false;
            };

            unsafe {
                let src = (page_virt + page_off as u64) as *const u8;
                core::ptr::copy_nonoverlapping(src, out.as_mut_ptr().add(copied), chunk);
            }
            copied += chunk;
        }
        true
    }

    fn read_unaligned<T: Copy>(&mut self, phys: u64) -> Option<T> {
        let sz = core::mem::size_of::<T>();
        if sz == 0 {
            return None;
        }
        if sz <= 64 {
            let mut buf = [0u8; 64];
            if !self.read_bytes(phys, &mut buf[..sz]) {
                return None;
            }
            Some(unsafe { read_unaligned(buf.as_ptr() as *const T) })
        } else {
            let mut val = core::mem::MaybeUninit::<T>::uninit();
            let dst = unsafe {
                core::slice::from_raw_parts_mut(val.as_mut_ptr() as *mut u8, sz)
            };
            if !self.read_bytes(phys, dst) {
                return None;
            }
            Some(unsafe { val.assume_init() })
        }
    }

    fn verify_checksum(&mut self, phys: u64, len: usize) -> bool {
        if len == 0 || len > 1024 * 1024 {
            return false;
        }
        let mut buf = [0u8; 256];
        let mut off = 0usize;
        let mut sum: u8 = 0;
        while off < len {
            let chunk = (len - off).min(buf.len());
            if !self.read_bytes(phys + off as u64, &mut buf[..chunk]) {
                return false;
            }
            for &b in &buf[..chunk] {
                sum = sum.wrapping_add(b);
            }
            off += chunk;
        }
        sum == 0
    }
}

impl Drop for AcpiReader<'_, '_, '_> {
    fn drop(&mut self) {
        if self.scratch_phys_page.take().is_some() {
            let scratch_va = VirtAddr::new(ACPI_SCRATCH_VIRT_BASE);
            let _ = self.mapping.unmap_page_with_allocator(self.allocator, scratch_va);
        }
    }
}

fn scan_rsdp_region(reader: &mut AcpiReader<'_, '_, '_>, start: u64, end: u64) -> Option<u64> {
    let mut addr = start & !0xFu64;
    while addr + 20 <= end {
        let Some(sig) = reader.read_unaligned::<[u8; 8]>(addr) else {
            // Advance past unmapped 4 KiB page instead of aborting the whole region.
            addr = (addr & !(PAGE_SIZE - 1)).saturating_add(PAGE_SIZE);
            continue;
        };
        if &sig == b"RSD PTR " && reader.verify_checksum(addr, 20) {
            if let Some(v1) = reader.read_unaligned::<RsdpV1>(addr) {
                if v1.revision >= 2 {
                    if let Some(v2) = reader.read_unaligned::<RsdpV2>(addr) {
                        let v2_len = (v2.length as usize).clamp(36, 128);
                        if reader.verify_checksum(addr, v2_len) {
                            return Some(addr);
                        }
                    }
                }
                return Some(addr);
            }
        }
        addr += 16;
    }
    None
}

fn find_rsdp(reader: &mut AcpiReader<'_, '_, '_>, rsdp_hint: Option<u64>) -> Option<u64> {
    if let Some(hint) = rsdp_hint {
        if let Some(sig) = reader.read_unaligned::<[u8; 8]>(hint) {
            if &sig == b"RSD PTR " && reader.verify_checksum(hint, 20) {
                return Some(hint);
            }
        }
    }

    // Legacy BIOS EBDA segment (stored at physical 0x040E << 4)
    if let Some(ebda_seg) = reader.read_unaligned::<u16>(0x040E) {
        let ebda_phys = (ebda_seg as u64) << 4;
        if ebda_phys >= 0x8_0000 && ebda_phys < 0xA_0000 {
            if let Some(found) = scan_rsdp_region(reader, ebda_phys, ebda_phys + 1024) {
                return Some(found);
            }
        }
    }

    // Main BIOS ROM area 0xE0000..0xFFFFF
    scan_rsdp_region(reader, 0x000E_0000, 0x0010_0000)
}

fn parse_madt(reader: &mut AcpiReader<'_, '_, '_>, phys: u64, len: usize, info: &mut AcpiPlatformInfo) {
    if len < core::mem::size_of::<SdtHeader>() + 8 {
        return;
    }
    let header_sz = core::mem::size_of::<SdtHeader>();
    if let Some(lapic32) = reader.read_unaligned::<u32>(phys + header_sz as u64) {
        info.lapic_phys = lapic32 as u64;
    }
    if let Some(flags) = reader.read_unaligned::<u32>(phys + header_sz as u64 + 4) {
        info.pcat_compat = (flags & 1) != 0;
    }

    let mut off = header_sz + 8;
    while off + 2 <= len {
        let Some(rec_type) = reader.read_unaligned::<u8>(phys + off as u64) else {
            break;
        };
        let Some(rec_len) = reader.read_unaligned::<u8>(phys + off as u64 + 1) else {
            break;
        };
        let rlen = rec_len as usize;
        if rlen < 2 || off + rlen > len {
            break;
        }

        match rec_type {
            // Type 0: Processor Local APIC
            0 if rlen >= 8 => {
                let acpi_id = reader.read_unaligned::<u8>(phys + off as u64 + 2).unwrap_or(0);
                let apic_id = reader.read_unaligned::<u8>(phys + off as u64 + 3).unwrap_or(0);
                let flags = reader.read_unaligned::<u32>(phys + off as u64 + 4).unwrap_or(0);
                let enabled = (flags & 1) != 0;
                let online_capable = (flags & 2) != 0;
                if enabled || online_capable {
                    info.enabled_cpu_count += 1;
                }
                if info.cpu_count < MAX_MADT_CPUS {
                    info.cpus[info.cpu_count] = MadtCpuEntry {
                        acpi_processor_id: acpi_id,
                        apic_id,
                        enabled,
                        online_capable,
                    };
                    info.cpu_count += 1;
                }
            }
            // Type 1: I/O APIC
            1 if rlen >= 12 => {
                let ioapic_id = reader.read_unaligned::<u8>(phys + off as u64 + 2).unwrap_or(0);
                let ioapic_phys = reader.read_unaligned::<u32>(phys + off as u64 + 4).unwrap_or(0);
                let gsi_base = reader.read_unaligned::<u32>(phys + off as u64 + 8).unwrap_or(0);
                if info.ioapic_count < MAX_MADT_IOAPICS {
                    info.ioapics[info.ioapic_count] = MadtIoApicEntry {
                        ioapic_id,
                        phys_addr: ioapic_phys,
                        gsi_base,
                    };
                    info.ioapic_count += 1;
                }
            }
            // Type 2: Interrupt Source Override
            2 if rlen >= 10 => {
                let bus_source = reader.read_unaligned::<u8>(phys + off as u64 + 2).unwrap_or(0);
                let irq_source = reader.read_unaligned::<u8>(phys + off as u64 + 3).unwrap_or(0);
                let gsi = reader.read_unaligned::<u32>(phys + off as u64 + 4).unwrap_or(0);
                let flags = reader.read_unaligned::<u16>(phys + off as u64 + 8).unwrap_or(0);
                if info.iso_count < MAX_MADT_ISOS {
                    info.isos[info.iso_count] = MadtIsoEntry {
                        bus_source,
                        irq_source,
                        gsi,
                        flags,
                    };
                    info.iso_count += 1;
                }
            }
            // Type 5: Local APIC Address Override (64-bit)
            5 if rlen >= 12 => {
                if let Some(addr64) = reader.read_unaligned::<u64>(phys + off as u64 + 4) {
                    if addr64 != 0 {
                        info.lapic_phys = addr64;
                    }
                }
            }
            _ => {}
        }

        off += rlen;
    }
}

fn parse_fadt(reader: &mut AcpiReader<'_, '_, '_>, phys: u64, len: usize, info: &mut AcpiPlatformInfo) {
    // ACPI Fixed ACPI Description Table offsets from start of table:
    // 46: SCI_INT (u16), 64: PM1a_CNT_BLK (u32), 68: PM1b_CNT_BLK (u32), 76: PM_TMR_BLK (u32), 108: CENTURY (u8)
    if len >= 80 {
        info.fadt_sci_irq = reader.read_unaligned::<u16>(phys + 46).unwrap_or(0);
        info.fadt_pm1a_cnt = reader.read_unaligned::<u32>(phys + 64).unwrap_or(0);
        info.fadt_pm1b_cnt = reader.read_unaligned::<u32>(phys + 68).unwrap_or(0);
        info.fadt_pm_tmr = reader.read_unaligned::<u32>(phys + 76).unwrap_or(0);
    }
    if len >= 109 {
        info.fadt_century_reg = reader.read_unaligned::<u8>(phys + 108).unwrap_or(0);
    }
}

fn parse_mcfg(reader: &mut AcpiReader<'_, '_, '_>, phys: u64, len: usize, info: &mut AcpiPlatformInfo) {
    // SdtHeader (36) + 8 bytes reserved -> 16-byte allocation records
    if len >= 36 + 8 + 16 {
        let base_rec = phys + 44;
        info.mcfg_ecam_base = reader.read_unaligned::<u64>(base_rec).unwrap_or(0);
        info.mcfg_start_bus = reader.read_unaligned::<u8>(base_rec + 10).unwrap_or(0);
        info.mcfg_end_bus = reader.read_unaligned::<u8>(base_rec + 11).unwrap_or(0);
    }
}

fn parse_hpet(reader: &mut AcpiReader<'_, '_, '_>, phys: u64, len: usize, info: &mut AcpiPlatformInfo) {
    // Offset 40 = Generic Address Structure (12 bytes), base address at +44
    if len >= 52 {
        info.hpet_phys = reader.read_unaligned::<u64>(phys + 44).unwrap_or(0);
    }
}

#[inline(never)]
pub fn init(
    mapping: &PhysicalMemoryMapping,
    allocator: &mut PhysicalFrameAllocator<'_>,
    rsdp_hint: Option<u64>,
) -> AcpiPlatformInfo {
    let mut info = AcpiPlatformInfo::EMPTY;
    let mut reader = AcpiReader::new(mapping, allocator);

    let Some(rsdp_phys) = find_rsdp(&mut reader, rsdp_hint) else {
        *ACPI_INFO.lock() = info;
        return info;
    };

    let Some(v1) = reader.read_unaligned::<RsdpV1>(rsdp_phys) else {
        *ACPI_INFO.lock() = info;
        return info;
    };

    info.discovered = true;
    info.revision = v1.revision;
    info.oem_id = v1.oem_id;
    info.rsdp_phys = rsdp_phys;

    let (root_sdt_phys, using_xsdt) = if v1.revision >= 2 {
        if let Some(v2) = reader.read_unaligned::<RsdpV2>(rsdp_phys) {
            if v2.xsdt_address != 0 {
                (v2.xsdt_address, true)
            } else {
                (v1.rsdt_address as u64, false)
            }
        } else {
            (v1.rsdt_address as u64, false)
        }
    } else {
        (v1.rsdt_address as u64, false)
    };

    info.root_sdt_phys = root_sdt_phys;
    info.using_xsdt = using_xsdt;

    if let Some(root_hdr) = reader.read_unaligned::<SdtHeader>(root_sdt_phys) {
        let hdr_sz = core::mem::size_of::<SdtHeader>();
        let root_len = root_hdr.length as usize;
        if root_len >= hdr_sz && root_len <= 256 * 1024 && reader.verify_checksum(root_sdt_phys, root_len) {
            let entry_sz = if using_xsdt { 8usize } else { 4usize };
            let entry_count = (root_len - hdr_sz) / entry_sz;

            for idx in 0..entry_count.min(MAX_ACPI_TABLES) {
                let entry_off = root_sdt_phys + hdr_sz as u64 + (idx * entry_sz) as u64;
                let table_phys = if using_xsdt {
                    reader.read_unaligned::<u64>(entry_off).unwrap_or(0)
                } else {
                    reader.read_unaligned::<u32>(entry_off).unwrap_or(0) as u64
                };

                if table_phys == 0 {
                    continue;
                }
                let Some(hdr) = reader.read_unaligned::<SdtHeader>(table_phys) else {
                    continue;
                };
                let tlen = hdr.length as usize;
                let checksum_ok = tlen >= hdr_sz && reader.verify_checksum(table_phys, tlen);

                if info.table_count < MAX_ACPI_TABLES {
                    info.tables[info.table_count] = AcpiTableSummary {
                        signature: hdr.signature,
                        phys_addr: table_phys,
                        length: hdr.length,
                        revision: hdr.revision,
                        checksum_ok,
                    };
                    info.table_count += 1;
                }

                if checksum_ok {
                    match &hdr.signature {
                        b"APIC" => parse_madt(&mut reader, table_phys, tlen, &mut info),
                        b"FACP" => parse_fadt(&mut reader, table_phys, tlen, &mut info),
                        b"MCFG" => parse_mcfg(&mut reader, table_phys, tlen, &mut info),
                        b"HPET" => parse_hpet(&mut reader, table_phys, tlen, &mut info),
                        _ => {}
                    }
                }
            }
        }
    }

    drop(reader);
    *ACPI_INFO.lock() = info;
    info
}

pub fn snapshot() -> AcpiPlatformInfo {
    *ACPI_INFO.lock()
}

pub fn system_reboot() -> ! {
    x86_64::instructions::interrupts::disable();
    unsafe {
        // 1. Classic 8042 keyboard controller CPU reset
        let mut kb_cmd: x86_64::instructions::port::Port<u8> =
            x86_64::instructions::port::Port::new(0x64);
        kb_cmd.write(0xFE);

        // 2. PCI / PIIX3 Reset Control Register (0xCF9)
        let mut reset_ctrl: x86_64::instructions::port::Port<u8> =
            x86_64::instructions::port::Port::new(0xCF9);
        reset_ctrl.write(0x02);
        reset_ctrl.write(0x06);

        // 3. QEMU ISA debug exit fallback
        let mut qemu_exit: x86_64::instructions::port::Port<u16> =
            x86_64::instructions::port::Port::new(0xF4);
        qemu_exit.write(0x00);
    }
    loop {
        x86_64::instructions::hlt();
    }
}

pub fn system_shutdown() -> ! {
    x86_64::instructions::interrupts::disable();
    let info = snapshot();
    unsafe {
        // 1. QEMU PIIX4 / Q35 & Bochs ACPI poweroff ports
        let mut qemu_pm: x86_64::instructions::port::Port<u16> =
            x86_64::instructions::port::Port::new(0x604);
        qemu_pm.write(0x2000);

        let mut bochs_pm: x86_64::instructions::port::Port<u16> =
            x86_64::instructions::port::Port::new(0xB004);
        bochs_pm.write(0x2000);

        // 2. VirtualBox ACPI shutdown port
        let mut vbox_pm: x86_64::instructions::port::Port<u16> =
            x86_64::instructions::port::Port::new(0x4004);
        vbox_pm.write(0x3400);

        // 3. ACPI FADT PM1a / PM1b SLP_EN (bit 13 = 0x2000, SLP_TYP5 = 0 / 5)
        if info.fadt_pm1a_cnt > 0 && info.fadt_pm1a_cnt <= 0xFFFF {
            let mut pm1a: x86_64::instructions::port::Port<u16> =
                x86_64::instructions::port::Port::new(info.fadt_pm1a_cnt as u16);
            pm1a.write(0x2000);
            pm1a.write(0x2000 | (5 << 10));
        }
        if info.fadt_pm1b_cnt > 0 && info.fadt_pm1b_cnt <= 0xFFFF {
            let mut pm1b: x86_64::instructions::port::Port<u16> =
                x86_64::instructions::port::Port::new(info.fadt_pm1b_cnt as u16);
            pm1b.write(0x2000);
            pm1b.write(0x2000 | (5 << 10));
        }
    }
    loop {
        x86_64::instructions::hlt();
    }
}

