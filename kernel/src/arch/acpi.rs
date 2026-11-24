use core::ptr::read_unaligned;
use spin::Mutex;

use crate::memory::vmm::PhysicalMemoryMapping;

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

unsafe fn phys_read_unaligned<T: Copy>(mapping: &PhysicalMemoryMapping, phys: u64) -> Option<T> {
    let v = mapping.translate(phys)?;
    Some(unsafe { read_unaligned(v.as_u64() as *const T) })
}

fn verify_checksum(mapping: &PhysicalMemoryMapping, phys: u64, len: usize) -> bool {
    if len == 0 || len > 1024 * 1024 {
        return false;
    }
    let Some(base) = mapping.translate(phys) else {
        return false;
    };
    let ptr = base.as_u64() as *const u8;
    let mut sum: u8 = 0;
    for i in 0..len {
        let b = unsafe { core::ptr::read_volatile(ptr.add(i)) };
        sum = sum.wrapping_add(b);
    }
    sum == 0
}

fn scan_rsdp_region(mapping: &PhysicalMemoryMapping, start: u64, end: u64) -> Option<u64> {
    let mut addr = start & !0xFu64;
    while addr + 20 <= end {
        let sig = phys_read_unaligned::<[u8; 8]>(mapping, addr)?;
        if &sig == b"RSD PTR " && verify_checksum(mapping, addr, 20) {
            let v1 = phys_read_unaligned::<RsdpV1>(mapping, addr)?;
            if v1.revision >= 2 {
                if let Some(v2) = phys_read_unaligned::<RsdpV2>(mapping, addr) {
                    let v2_len = (v2.length as usize).clamp(36, 128);
                    if verify_checksum(mapping, addr, v2_len) {
                        return Some(addr);
                    }
                }
            }
            return Some(addr);
        }
        addr += 16;
    }
    None
}

fn find_rsdp(mapping: &PhysicalMemoryMapping, rsdp_hint: Option<u64>) -> Option<u64> {
    if let Some(hint) = rsdp_hint {
        if let Some(sig) = unsafe { phys_read_unaligned::<[u8; 8]>(mapping, hint) } {
            if &sig == b"RSD PTR " && verify_checksum(mapping, hint, 20) {
                return Some(hint);
            }
        }
    }

    // Legacy BIOS EBDA segment (stored at physical 0x040E << 4)
    if let Some(ebda_seg) = unsafe { phys_read_unaligned::<u16>(mapping, 0x040E) } {
        let ebda_phys = (ebda_seg as u64) << 4;
        if ebda_phys >= 0x8_0000 && ebda_phys < 0xA_0000 {
            if let Some(found) = scan_rsdp_region(mapping, ebda_phys, ebda_phys + 1024) {
                return Some(found);
            }
        }
    }

    // Main BIOS ROM area 0xE0000..0xFFFFF
    scan_rsdp_region(mapping, 0x000E_0000, 0x0010_0000)
}

fn parse_madt(mapping: &PhysicalMemoryMapping, phys: u64, len: usize, info: &mut AcpiPlatformInfo) {
    if len < core::mem::size_of::<SdtHeader>() + 8 {
        return;
    }
    let header_sz = core::mem::size_of::<SdtHeader>();
    if let Some(lapic32) = unsafe { phys_read_unaligned::<u32>(mapping, phys + header_sz as u64) } {
        info.lapic_phys = lapic32 as u64;
    }
    if let Some(flags) = unsafe { phys_read_unaligned::<u32>(mapping, phys + header_sz as u64 + 4) } {
        info.pcat_compat = (flags & 1) != 0;
    }

    let mut off = header_sz + 8;
    while off + 2 <= len {
        let Some(rec_type) = unsafe { phys_read_unaligned::<u8>(mapping, phys + off as u64) } else {
            break;
        };
        let Some(rec_len) = unsafe { phys_read_unaligned::<u8>(mapping, phys + off as u64 + 1) } else {
            break;
        };
        let rlen = rec_len as usize;
        if rlen < 2 || off + rlen > len {
            break;
        }

        match rec_type {
            // Type 0: Processor Local APIC
            0 if rlen >= 8 => {
                let acpi_id = unsafe { phys_read_unaligned::<u8>(mapping, phys + off as u64 + 2) }.unwrap_or(0);
                let apic_id = unsafe { phys_read_unaligned::<u8>(mapping, phys + off as u64 + 3) }.unwrap_or(0);
                let flags = unsafe { phys_read_unaligned::<u32>(mapping, phys + off as u64 + 4) }.unwrap_or(0);
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
                let ioapic_id = unsafe { phys_read_unaligned::<u8>(mapping, phys + off as u64 + 2) }.unwrap_or(0);
                let ioapic_phys = unsafe { phys_read_unaligned::<u32>(mapping, phys + off as u64 + 4) }.unwrap_or(0);
                let gsi_base = unsafe { phys_read_unaligned::<u32>(mapping, phys + off as u64 + 8) }.unwrap_or(0);
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
                let bus_source = unsafe { phys_read_unaligned::<u8>(mapping, phys + off as u64 + 2) }.unwrap_or(0);
                let irq_source = unsafe { phys_read_unaligned::<u8>(mapping, phys + off as u64 + 3) }.unwrap_or(0);
                let gsi = unsafe { phys_read_unaligned::<u32>(mapping, phys + off as u64 + 4) }.unwrap_or(0);
                let flags = unsafe { phys_read_unaligned::<u16>(mapping, phys + off as u64 + 8) }.unwrap_or(0);
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
                if let Some(addr64) = unsafe { phys_read_unaligned::<u64>(mapping, phys + off as u64 + 4) } {
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

fn parse_fadt(mapping: &PhysicalMemoryMapping, phys: u64, len: usize, info: &mut AcpiPlatformInfo) {
    // ACPI Fixed ACPI Description Table offsets from start of table:
    // 46: SCI_INT (u16), 64: PM1a_CNT_BLK (u32), 68: PM1b_CNT_BLK (u32), 76: PM_TMR_BLK (u32), 108: CENTURY (u8)
    if len >= 80 {
        info.fadt_sci_irq = unsafe { phys_read_unaligned::<u16>(mapping, phys + 46) }.unwrap_or(0);
        info.fadt_pm1a_cnt = unsafe { phys_read_unaligned::<u32>(mapping, phys + 64) }.unwrap_or(0);
        info.fadt_pm1b_cnt = unsafe { phys_read_unaligned::<u32>(mapping, phys + 68) }.unwrap_or(0);
        info.fadt_pm_tmr = unsafe { phys_read_unaligned::<u32>(mapping, phys + 76) }.unwrap_or(0);
    }
    if len >= 109 {
        info.fadt_century_reg = unsafe { phys_read_unaligned::<u8>(mapping, phys + 108) }.unwrap_or(0);
    }
}

fn parse_mcfg(mapping: &PhysicalMemoryMapping, phys: u64, len: usize, info: &mut AcpiPlatformInfo) {
    // SdtHeader (36) + 8 bytes reserved -> 16-byte allocation records
    if len >= 36 + 8 + 16 {
        let base_rec = phys + 44;
        info.mcfg_ecam_base = unsafe { phys_read_unaligned::<u64>(mapping, base_rec) }.unwrap_or(0);
        info.mcfg_start_bus = unsafe { phys_read_unaligned::<u8>(mapping, base_rec + 10) }.unwrap_or(0);
        info.mcfg_end_bus = unsafe { phys_read_unaligned::<u8>(mapping, base_rec + 11) }.unwrap_or(0);
    }
}

fn parse_hpet(mapping: &PhysicalMemoryMapping, phys: u64, len: usize, info: &mut AcpiPlatformInfo) {
    // Offset 40 = Generic Address Structure (12 bytes), base address at +44
    if len >= 52 {
        info.hpet_phys = unsafe { phys_read_unaligned::<u64>(mapping, phys + 44) }.unwrap_or(0);
    }
}

pub fn init(mapping: &PhysicalMemoryMapping, rsdp_hint: Option<u64>) -> AcpiPlatformInfo {
    let mut info = AcpiPlatformInfo::EMPTY;

    let Some(rsdp_phys) = find_rsdp(mapping, rsdp_hint) else {
        *ACPI_INFO.lock() = info;
        return info;
    };

    let Some(v1) = (unsafe { phys_read_unaligned::<RsdpV1>(mapping, rsdp_phys) }) else {
        *ACPI_INFO.lock() = info;
        return info;
    };

    info.discovered = true;
    info.revision = v1.revision;
    info.oem_id = v1.oem_id;
    info.rsdp_phys = rsdp_phys;

    let (root_sdt_phys, using_xsdt) = if v1.revision >= 2 {
        if let Some(v2) = unsafe { phys_read_unaligned::<RsdpV2>(mapping, rsdp_phys) } {
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

    if let Some(root_hdr) = unsafe { phys_read_unaligned::<SdtHeader>(mapping, root_sdt_phys) } {
        let hdr_sz = core::mem::size_of::<SdtHeader>();
        let root_len = root_hdr.length as usize;
        if root_len >= hdr_sz && root_len <= 256 * 1024 && verify_checksum(mapping, root_sdt_phys, root_len) {
            let entry_sz = if using_xsdt { 8usize } else { 4usize };
            let entry_count = (root_len - hdr_sz) / entry_sz;

            for idx in 0..entry_count.min(MAX_ACPI_TABLES) {
                let entry_off = root_sdt_phys + hdr_sz as u64 + (idx * entry_sz) as u64;
                let table_phys = if using_xsdt {
                    unsafe { phys_read_unaligned::<u64>(mapping, entry_off) }.unwrap_or(0)
                } else {
                    unsafe { phys_read_unaligned::<u32>(mapping, entry_off) }.unwrap_or(0) as u64
                };

                if table_phys == 0 {
                    continue;
                }
                let Some(hdr) = (unsafe { phys_read_unaligned::<SdtHeader>(mapping, table_phys) }) else {
                    continue;
                };
                let tlen = hdr.length as usize;
                let checksum_ok = tlen >= hdr_sz && verify_checksum(mapping, table_phys, tlen);

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
                        b"APIC" => parse_madt(mapping, table_phys, tlen, &mut info),
                        b"FACP" => parse_fadt(mapping, table_phys, tlen, &mut info),
                        b"MCFG" => parse_mcfg(mapping, table_phys, tlen, &mut info),
                        b"HPET" => parse_hpet(mapping, table_phys, tlen, &mut info),
                        _ => {}
                    }
                }
            }
        }
    }

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
