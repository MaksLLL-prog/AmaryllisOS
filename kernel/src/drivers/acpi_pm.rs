use core::arch::asm;

use crate::arch::acpi;
use crate::freebsd_kpi::Mtx;
use crate::memory::vmm::PhysicalMemoryMapping;

/// FreeBSD `acpi_pm` + `acpi_battery` + `acpi_tz` + `acpi_ec` + `coretemp` driver.

pub const MAX_ACPI_PM_DEVICES: usize = 16;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AcpiPmDevKind {
    Battery,
    AcAdapter,
    EmbeddedController,
    ThermalZone,
    PowerButton,
    SleepButton,
    PciRootBridge,
    Other,
}

impl AcpiPmDevKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Battery => "Battery",
            Self::AcAdapter => "AC Adapter",
            Self::EmbeddedController => "EmbeddedCtrl",
            Self::ThermalZone => "ThermalZone",
            Self::PowerButton => "PowerButton",
            Self::SleepButton => "SleepButton",
            Self::PciRootBridge => "PCIe Root",
            Self::Other => "ACPI Device",
        }
    }
}

#[derive(Clone, Copy)]
pub struct AcpiPmDeviceInfo {
    pub valid: bool,
    pub kind: AcpiPmDevKind,
    pub name: [u8; 4],
    pub hid: [u8; 16],
    pub hid_len: u8,
}

impl AcpiPmDeviceInfo {
    pub const EMPTY: Self = Self {
        valid: false,
        kind: AcpiPmDevKind::Other,
        name: [b' '; 4],
        hid: [0; 16],
        hid_len: 0,
    };

    pub fn name_str(&self) -> &str {
        core::str::from_utf8(&self.name).unwrap_or("????").trim()
    }

    pub fn hid_str(&self) -> &str {
        core::str::from_utf8(&self.hid[..self.hid_len as usize]).unwrap_or("")
    }
}

#[derive(Clone, Copy)]
pub struct AcpiPowerSnapshot {
    pub initialized: bool,
    pub dsdt_bytes: usize,
    pub ssdt_count: usize,
    pub pm_profile: u8,
    pub sci_irq: u16,
    pub pm1a_cnt: u32,
    pub pm1b_cnt: u32,
    pub pm_tmr: u32,
    pub gpe0_blk: u32,
    pub s5_found: bool,
    pub s5_slp_typa: u8,
    pub s5_slp_typb: u8,
    pub ec_present: bool,
    pub ec_responsive: bool,
    pub battery_count: usize,
    pub battery_present: bool,
    pub battery_percent: Option<u8>,
    pub ac_present: bool,
    pub ac_online: Option<bool>,
    pub power_button_present: bool,
    pub thermal_zone_count: usize,
    pub cpu_dts_supported: bool,
    pub cpu_temp_celsius: Option<i16>,
    pub tj_max_celsius: i16,
    pub device_count: usize,
    pub devices: [AcpiPmDeviceInfo; MAX_ACPI_PM_DEVICES],
}

impl AcpiPowerSnapshot {
    pub const EMPTY: Self = Self {
        initialized: false,
        dsdt_bytes: 0,
        ssdt_count: 0,
        pm_profile: 0,
        sci_irq: 0,
        pm1a_cnt: 0,
        pm1b_cnt: 0,
        pm_tmr: 0,
        gpe0_blk: 0,
        s5_found: false,
        s5_slp_typa: 5,
        s5_slp_typb: 5,
        ec_present: false,
        ec_responsive: false,
        battery_count: 0,
        battery_present: false,
        battery_percent: None,
        ac_present: false,
        ac_online: None,
        power_button_present: false,
        thermal_zone_count: 0,
        cpu_dts_supported: false,
        cpu_temp_celsius: None,
        tj_max_celsius: 100,
        device_count: 0,
        devices: [AcpiPmDeviceInfo::EMPTY; MAX_ACPI_PM_DEVICES],
    };

    pub const fn pm_profile_name(&self) -> &'static str {
        match self.pm_profile {
            0 => "Unspecified",
            1 => "Desktop",
            2 => "Mobile / Laptop",
            3 => "Workstation",
            4 => "Enterprise Server",
            5 => "SOHO Server",
            6 => "Appliance PC",
            7 => "Performance Server",
            8 => "Tablet",
            _ => "Other",
        }
    }
}

static ACPI_PM_STATE: Mtx<AcpiPowerSnapshot> = Mtx::new("acpi_pm", AcpiPowerSnapshot::EMPTY);

#[inline]
unsafe fn inb_port(port: u16) -> u8 {
    let v: u8;
    unsafe {
        asm!("in al, dx", in("dx") port, out("al") v, options(nomem, nostack, preserves_flags));
    }
    v
}

#[inline]
fn decode_eisa_id(val: u32) -> [u8; 7] {
    // Compressed ACPI EISAID (big-endian 16-bit vendor + 16-bit product bytes)
    let b = val.to_le_bytes();
    let v0 = (((b[0] >> 2) & 0x1F) + b'@') as u8;
    let v1 = ((((b[0] & 0x03) << 3) | ((b[1] >> 5) & 0x07)) + b'@') as u8;
    let v2 = ((b[1] & 0x1F) + b'@') as u8;
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    [
        v0,
        v1,
        v2,
        HEX[((b[2] >> 4) & 0x0F) as usize],
        HEX[(b[2] & 0x0F) as usize],
        HEX[((b[3] >> 4) & 0x0F) as usize],
        HEX[(b[3] & 0x0F) as usize],
    ]
}

fn parse_aml_integer(aml: &[u8], pos: usize) -> Option<(u64, usize)> {
    let op = *aml.get(pos)?;
    match op {
        0x00 => Some((0, 1)), // ZeroOp
        0x01 => Some((1, 1)), // OneOp
        0xFF => Some((u64::MAX, 1)),
        0x0A => Some((*aml.get(pos + 1)? as u64, 2)), // BytePrefix
        0x0B => {
            let lo = *aml.get(pos + 1)?;
            let hi = *aml.get(pos + 2)?;
            Some((u16::from_le_bytes([lo, hi]) as u64, 3))
        }
        0x0C => {
            let s = aml.get(pos + 1..pos + 5)?;
            Some((u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as u64, 5))
        }
        0x0E => {
            let s = aml.get(pos + 1..pos + 9)?;
            Some((
                u64::from_le_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]),
                9,
            ))
        }
        _ => None,
    }
}

fn parse_pkg_len(aml: &[u8], pos: usize) -> Option<(usize, usize)> {
    let lead = *aml.get(pos)?;
    let follow = (lead >> 6) as usize;
    if follow == 0 {
        return Some(((lead & 0x3F) as usize, 1));
    }
    if pos + 1 + follow > aml.len() {
        return None;
    }
    let mut len = (lead & 0x0F) as usize;
    for i in 0..follow {
        len |= (aml[pos + 1 + i] as usize) << (4 + i * 8);
    }
    Some((len, 1 + follow))
}

#[inline]
fn is_name_char(b: u8) -> bool {
    b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_'
}

fn find_s5_package(aml: &[u8]) -> Option<(u8, u8)> {
    // Search for NameOp (0x08) "_S5_" followed by PackageOp (0x12)
    for i in 0..aml.len().saturating_sub(12) {
        if aml[i] == 0x08 && &aml[i + 1..i + 5] == b"_S5_" && aml[i + 5] == 0x12 {
            let (_, plen_sz) = parse_pkg_len(aml, i + 6)?;
            let cursor = i + 6 + plen_sz + 1; // +1 skips NumElements byte
            let (typa, sz1) = parse_aml_integer(aml, cursor)?;
            let typb = parse_aml_integer(aml, cursor + sz1)
                .map(|v| v.0)
                .unwrap_or(typa);
            return Some(((typa & 0x07) as u8, (typb & 0x07) as u8));
        }
        // Fallback: "_S5_" directly followed by PackageOp (if prefixed by Root/Scope)
        if &aml[i..i + 4] == b"_S5_" && aml[i + 4] == 0x12 {
            if let Some((_, plen_sz)) = parse_pkg_len(aml, i + 5) {
                let cursor = i + 5 + plen_sz + 1;
                if let Some((typa, sz1)) = parse_aml_integer(aml, cursor) {
                    let typb = parse_aml_integer(aml, cursor + sz1)
                        .map(|v| v.0)
                        .unwrap_or(typa);
                    return Some(((typa & 0x07) as u8, (typb & 0x07) as u8));
                }
            }
        }
    }
    None
}

fn push_pm_device(snap: &mut AcpiPowerSnapshot, kind: AcpiPmDevKind, name: [u8; 4], hid: &str) {
    // Avoid duplicates by (kind, hid, name)
    for i in 0..snap.device_count {
        let d = &snap.devices[i];
        if d.valid && d.kind == kind && d.hid_str() == hid && d.name == name {
            return;
        }
    }
    if snap.device_count < MAX_ACPI_PM_DEVICES {
        let mut entry = AcpiPmDeviceInfo::EMPTY;
        entry.valid = true;
        entry.kind = kind;
        entry.name = name;
        let hb = hid.as_bytes();
        let hl = hb.len().min(entry.hid.len());
        entry.hid[..hl].copy_from_slice(&hb[..hl]);
        entry.hid_len = hl as u8;
        snap.devices[snap.device_count] = entry;
        snap.device_count += 1;
    }

    match kind {
        AcpiPmDevKind::Battery => {
            snap.battery_count += 1;
            snap.battery_present = true;
        }
        AcpiPmDevKind::AcAdapter => snap.ac_present = true,
        AcpiPmDevKind::EmbeddedController => snap.ec_present = true,
        AcpiPmDevKind::ThermalZone => snap.thermal_zone_count += 1,
        AcpiPmDevKind::PowerButton => snap.power_button_present = true,
        _ => {}
    }
}

fn classify_hid(hid: &str) -> Option<AcpiPmDevKind> {
    match hid {
        "PNP0C0A" => Some(AcpiPmDevKind::Battery),
        "ACPI0003" => Some(AcpiPmDevKind::AcAdapter),
        "PNP0C09" => Some(AcpiPmDevKind::EmbeddedController),
        "PNP0C0C" => Some(AcpiPmDevKind::PowerButton),
        "PNP0C0E" => Some(AcpiPmDevKind::SleepButton),
        "PNP0A03" | "PNP0A08" => Some(AcpiPmDevKind::PciRootBridge),
        _ => None,
    }
}

fn nearest_preceding_name(aml: &[u8], pos: usize) -> [u8; 4] {
    let start = pos.saturating_sub(24);
    for idx in (start..pos.saturating_sub(4)).rev() {
        // DeviceOp 0x5B 0x82 + PkgLen + NameSeg
        if idx + 6 < pos && aml[idx] == 0x5B && aml[idx + 1] == 0x82 {
            if let Some((_, plen_sz)) = parse_pkg_len(aml, idx + 2) {
                let npos = idx + 2 + plen_sz;
                if npos + 4 <= aml.len() && aml[npos..npos + 4].iter().all(|&b| is_name_char(b)) {
                    return [aml[npos], aml[npos + 1], aml[npos + 2], aml[npos + 3]];
                }
            }
        }
    }
    *b"ACPI"
}

fn scan_aml_table(aml: &[u8], snap: &mut AcpiPowerSnapshot) {
    if aml.len() <= 36 {
        return;
    }
    let body = &aml[36..];

    if !snap.s5_found {
        if let Some((a, b)) = find_s5_package(body) {
            snap.s5_found = true;
            snap.s5_slp_typa = a;
            snap.s5_slp_typb = b;
            acpi::set_s5_sleep_types(a, b);
        }
    }

    let mut i = 0usize;
    while i + 6 < body.len() {
        // ThermalZoneOp: 0x5B 0x85 + PkgLen + NameSeg
        if body[i] == 0x5B && body[i + 1] == 0x85 {
            if let Some((_, plen_sz)) = parse_pkg_len(body, i + 2) {
                let npos = i + 2 + plen_sz;
                if npos + 4 <= body.len() && body[npos..npos + 4].iter().all(|&b| is_name_char(b)) {
                    let name = [body[npos], body[npos + 1], body[npos + 2], body[npos + 3]];
                    push_pm_device(snap, AcpiPmDevKind::ThermalZone, name, "THERMAL");
                }
            }
        }

        // NameOp (0x08) "_HID" followed by DWordPrefix (0x0C) EISAID or StringPrefix (0x0D)
        if body[i] == 0x08 && &body[i + 1..i + 5] == b"_HID" {
            let val_pos = i + 5;
            if val_pos + 5 <= body.len() && body[val_pos] == 0x0C {
                let eisa = u32::from_le_bytes([
                    body[val_pos + 1],
                    body[val_pos + 2],
                    body[val_pos + 3],
                    body[val_pos + 4],
                ]);
                let decoded = decode_eisa_id(eisa);
                if let Ok(hid) = core::str::from_utf8(&decoded) {
                    if let Some(kind) = classify_hid(hid) {
                        let name = nearest_preceding_name(body, i);
                        push_pm_device(snap, kind, name, hid);
                    }
                }
            } else if val_pos + 2 <= body.len() && body[val_pos] == 0x0D {
                let mut end = val_pos + 1;
                while end < body.len() && end < val_pos + 16 && body[end] != 0 {
                    end += 1;
                }
                if let Ok(hid) = core::str::from_utf8(&body[val_pos + 1..end]) {
                    if let Some(kind) = classify_hid(hid) {
                        let name = nearest_preceding_name(body, i);
                        push_pm_device(snap, kind, name, hid);
                    }
                }
            }
        }
        i += 1;
    }
}

fn read_cpu_dts_temperature() -> (bool, Option<i16>, i16) {
    let max_leaf: u32;
    let eax_6: u32;
    unsafe {
        asm!(
            "push rbx",
            "xor eax, eax",
            "cpuid",
            "pop rbx",
            out("eax") max_leaf,
            out("ecx") _,
            out("edx") _,
            options(nomem, preserves_flags)
        );
    }
    if max_leaf < 6 {
        return (false, None, 100);
    }
    unsafe {
        asm!(
            "push rbx",
            "mov eax, 6",
            "xor ecx, ecx",
            "cpuid",
            "pop rbx",
            out("eax") eax_6,
            out("ecx") _,
            out("edx") _,
            options(nomem, preserves_flags)
        );
    }
    if (eax_6 & 1) == 0 {
        return (false, None, 100);
    }

    // IA32_TEMPERATURE_TARGET (0x1A2) bits [23:16] = TjMax; default 100 C if 0
    let target_msr = crate::arch::percpu::read_msr(0x1A2);
    let tj_max = ((target_msr >> 16) & 0xFF) as i16;
    let tj = if tj_max >= 50 && tj_max <= 125 { tj_max } else { 100 };

    // IA32_THERM_STATUS (0x19C): bit 31 = Reading Valid, bits [22:16] = Digital Readout (delta below TjMax)
    let therm_sts = crate::arch::percpu::read_msr(0x19C);
    if (therm_sts & (1 << 31)) != 0 {
        let delta = ((therm_sts >> 16) & 0x7F) as i16;
        (true, Some((tj - delta).clamp(-20, 125)), tj)
    } else {
        (true, None, tj)
    }
}

fn probe_ec_nonblocking(ec_present: bool) -> bool {
    if !ec_present {
        return false;
    }
    // Status port 0x66: 0xFF means no EC hardware behind port
    let sts = unsafe { inb_port(0x66) };
    sts != 0xFF
}

pub fn init(_mapping: Option<&PhysicalMemoryMapping>) -> AcpiPowerSnapshot {
    let acpi_info = acpi::snapshot();
    let (dsdt, ssdts) = acpi::copy_aml_tables();

    let mut snap = AcpiPowerSnapshot::EMPTY;
    snap.initialized = acpi_info.discovered;
    snap.dsdt_bytes = dsdt.len();
    snap.ssdt_count = ssdts.len();
    snap.pm_profile = acpi_info.fadt_pm_profile;
    snap.sci_irq = acpi_info.fadt_sci_irq;
    snap.pm1a_cnt = acpi_info.fadt_pm1a_cnt;
    snap.pm1b_cnt = acpi_info.fadt_pm1b_cnt;
    snap.pm_tmr = acpi_info.fadt_pm_tmr;
    snap.gpe0_blk = acpi_info.fadt_gpe0_blk;

    if !dsdt.is_empty() {
        scan_aml_table(&dsdt, &mut snap);
    }
    for ssdt in &ssdts {
        scan_aml_table(ssdt, &mut snap);
    }

    snap.ec_responsive = probe_ec_nonblocking(snap.ec_present);
    let (dts_ok, temp_c, tj_max) = read_cpu_dts_temperature();
    snap.cpu_dts_supported = dts_ok;
    snap.cpu_temp_celsius = temp_c;
    snap.tj_max_celsius = tj_max;

    // If AML reported AC adapter without a battery, mark AC online on desktop profile
    if !snap.battery_present && (snap.ac_present || snap.pm_profile == 1 || snap.pm_profile == 0) {
        snap.ac_online = Some(true);
    }

    *ACPI_PM_STATE.lock() = snap;
    snap
}

pub fn refresh() -> AcpiPowerSnapshot {
    let mut sc = ACPI_PM_STATE.lock();
    sc.ec_responsive = probe_ec_nonblocking(sc.ec_present);
    let (dts_ok, temp_c, tj_max) = read_cpu_dts_temperature();
    sc.cpu_dts_supported = dts_ok;
    sc.cpu_temp_celsius = temp_c;
    sc.tj_max_celsius = tj_max;
    *sc
}

pub fn is_ready() -> bool {
    ACPI_PM_STATE.lock().initialized
}

pub fn snapshot() -> AcpiPowerSnapshot {
    *ACPI_PM_STATE.lock()
}

pub fn shutdown() -> ! {
    acpi::system_shutdown()
}

pub fn reboot() -> ! {
    acpi::system_reboot()
}
