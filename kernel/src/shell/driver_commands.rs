use crate::console::Console;
use crate::drivers::registry::{self, DriverStatus};
use crate::graphics::Color;

const WHITE: Color = (235, 235, 240);
const GREEN: Color = (40, 180, 120);
const GRAY: Color = (140, 140, 150);
const YELLOW: Color = (220, 190, 90);
const RED: Color = (220, 90, 90);

pub fn cmd_drivers(console: &mut Console<'_>, arguments: &str) {
    let filter_active = arguments.trim() == "--active";

    let total = registry::count();
    console.write("\nKernel Unified Driver Registry:\n", GREEN);
    console.write("ID  CLASS     STATUS       BUS                   NAME / DETAILS\n", GRAY);
    console.write("------------------------------------------------------------------------\n", GRAY);

    if total == 0 {
        console.write("No drivers registered.\n", YELLOW);
        return;
    }

    let mut active_count = 0usize;
    let mut not_present_count = 0usize;
    let mut failed_count = 0usize;

    for idx in 0..total {
        let Some(entry) = registry::get(idx) else {
            continue;
        };

        match entry.status {
            DriverStatus::Active => active_count += 1,
            DriverStatus::NotPresent => not_present_count += 1,
            DriverStatus::Failed => failed_count += 1,
            DriverStatus::Registered => {}
        }

        if filter_active && entry.status != DriverStatus::Active {
            continue;
        }

        write_padded_decimal(console, entry.id as u64, 2);
        console.write("  ", WHITE);

        write_padded_str(console, entry.class.name(), 8, WHITE);
        console.write("  ", WHITE);

        let status_color = match entry.status {
            DriverStatus::Active => GREEN,
            DriverStatus::Registered => WHITE,
            DriverStatus::NotPresent => YELLOW,
            DriverStatus::Failed => RED,
        };
        write_padded_str(console, entry.status.name(), 11, status_color);
        console.write("  ", WHITE);

        let mut bus_buf = [0u8; 32];
        let bus_str = entry.bus.format_short(&mut bus_buf);
        write_padded_str(console, bus_str, 20, GRAY);
        console.write("  ", WHITE);

        console.write(entry.name, WHITE);
        let details = entry.details_str();
        if !details.is_empty() {
            console.write(" (", GRAY);
            console.write(details, GRAY);
            console.write(")", GRAY);
        }
        console.write("\n", WHITE);
    }

    console.write("\nSummary: ", GRAY);
    write_decimal(console, total as u64);
    console.write(" registered, ", GRAY);
    write_decimal(console, active_count as u64);
    console.write(" active, ", GREEN);
    write_decimal(console, not_present_count as u64);
    console.write(" absent, ", YELLOW);
    write_decimal(console, failed_count as u64);
    console.write(" failed\n", RED);
}

fn write_padded_str(console: &mut Console<'_>, s: &str, width: usize, color: Color) {
    console.write(s, color);
    if s.len() < width {
        for _ in 0..(width - s.len()) {
            console.write(" ", color);
        }
    }
}

fn write_padded_decimal(console: &mut Console<'_>, val: u64, width: usize) {
    let mut digits = 1usize;
    let mut tmp = val;
    while tmp >= 10 {
        tmp /= 10;
        digits += 1;
    }
    if digits < width {
        for _ in 0..(width - digits) {
            console.write(" ", WHITE);
        }
    }
    write_decimal(console, val);
}

fn write_decimal(console: &mut Console<'_>, mut number: u64) {
    if number == 0 {
        console.write("0", WHITE);
        return;
    }
    let mut buffer = [0u8; 20];
    let mut index = buffer.len();
    while number > 0 {
        index -= 1;
        buffer[index] = b'0' + (number % 10) as u8;
        number /= 10;
    }
    if let Ok(text) = core::str::from_utf8(&buffer[index..]) {
        console.write(text, WHITE);
    }
}

fn write_hex64(console: &mut Console<'_>, val: u64) {
    let hex = alloc::format!("0x{:x}", val);
    console.write(&hex, WHITE);
}

pub fn cmd_acpi(console: &mut Console<'_>) {
    let info = crate::arch::acpi::snapshot();
    console.write("\nACPI Firmware Tables:\n", GREEN);
    if !info.discovered {
        console.write("  ACPI RSDP not found.\n", YELLOW);
        return;
    }
    console.write("  OEM ID:    ", GRAY);
    console.write(info.oem_str(), WHITE);
    console.write("\n  RSDP phys: ", GRAY);
    write_hex64(console, info.rsdp_phys);
    console.write("\n  Root SDT:  ", GRAY);
    console.write(if info.using_xsdt { "XSDT " } else { "RSDT " }, WHITE);
    write_hex64(console, info.root_sdt_phys);
    console.write("\n  MADT CPUs: ", GRAY);
    write_decimal(console, info.cpu_count as u64);
    console.write(" | IOAPICs: ", GRAY);
    write_decimal(console, info.ioapic_count as u64);
    console.write(" | ISOs: ", GRAY);
    write_decimal(console, info.iso_count as u64);
    if info.mcfg_ecam_base != 0 {
        console.write("\n  PCIe ECAM: ", GRAY);
        write_hex64(console, info.mcfg_ecam_base);
    }
    if info.hpet_phys != 0 {
        console.write("\n  HPET phys: ", GRAY);
        write_hex64(console, info.hpet_phys);
    }
    console.write("\n  Tables:\n", GREEN);
    for i in 0..info.table_count {
        let t = info.tables[i];
        let line = alloc::format!(
            "    {}  rev={}  len={:<5}  phys=0x{:x}  chk={}\n",
            t.sig_str(),
            t.revision,
            t.length,
            t.phys_addr,
            if t.checksum_ok { "OK" } else { "BAD" }
        );
        console.write(&line, WHITE);
    }
}

pub fn cmd_apic(console: &mut Console<'_>) {
    let snap = crate::arch::apic::snapshot();
    let acpi = crate::arch::acpi::snapshot();
    let percpu = crate::arch::percpu::bsp_snapshot();
    let smp = crate::arch::smp::summary();
    let percpu_list = crate::arch::percpu::snapshot_all();
    console.write("\nLocal APIC, IOAPIC & SMP Topology:\n", GREEN);
    console.write("  CPU Vendor:    ", GRAY);
    console.write(snap.vendor_str(), WHITE);
    console.write("\n  CPU Brand:     ", GRAY);
    console.write(snap.brand_str(), WHITE);
    console.write("\n  LAPIC Status:  ", GRAY);
    console.write(
        if snap.lapic_enabled { "ENABLED\n" } else { "DISABLED\n" },
        if snap.lapic_enabled { GREEN } else { YELLOW },
    );
    let details = alloc::format!(
        "  BSP APIC ID:   {}  Version: 0x{:02x}  MaxLVT: {}  SVR: 0x{:03x}\n  LAPIC phys:    0x{:x}  virt: 0x{:x}\n  x2APIC CPUID:  {}  PerCpu GS_KERN: 0x{:x}\n  SMP Status:    {} online CPU(s) (BSP + {} APs, {} ACPI detected)  TLB Shootdowns: {}\n  LAPIC Timer:   {} ({} Hz, {} ticks/ms, init=0x{:x}, irqs={})\n",
        snap.bsp_apic_id,
        snap.lapic_version,
        snap.lapic_max_lvt,
        snap.svr_value,
        snap.lapic_phys,
        snap.lapic_virt,
        if snap.x2apic_supported { "yes" } else { "no" },
        crate::arch::percpu::read_msr(crate::arch::percpu::IA32_KERNEL_GS_BASE_MSR),
        smp.online_cpus,
        smp.ap_booted,
        snap.detected_cores,
        smp.tlb_shootdowns,
        if snap.timer_active { "ACTIVE" } else { "INACTIVE" },
        snap.timer_hz,
        snap.timer_ticks_per_ms,
        snap.timer_init_count,
        crate::klog::stats().apic_timer_irqs,
    );
    console.write(&details, WHITE);
    let _ = percpu;

    console.write("  Online Per-CPU Cores:\n", GRAY);
    for cpu in &percpu_list {
        if !cpu.online {
            continue;
        }
        let line = alloc::format!(
            "    CPU{} apic_id={} role={} timer_irqs={} ipis={} tlb_inv={} kthread_slices={}\n",
            cpu.cpu_id,
            cpu.apic_id,
            if cpu.cpu_id == 0 { "BSP" } else { "AP " },
            cpu.timer_irqs,
            cpu.ipi_count,
            cpu.tlb_shootdowns,
            cpu.kthread_slices,
        );
        console.write(&line, if cpu.cpu_id == 0 { WHITE } else { GREEN });
    }

    for i in 0..snap.ioapic_count {
        let io = snap.ioapics[i];
        let line = alloc::format!(
            "  IOAPIC[{}]: id={} phys=0x{:x} gsi_base={} ver=0x{:02x} redirs={}\n",
            i, io.id, io.phys_addr, io.gsi_base, io.version, io.max_redirs
        );
        console.write(&line, WHITE);
    }
    if acpi.cpu_count > 0 {
        console.write("  MADT Processors:\n", GRAY);
        for i in 0..acpi.cpu_count.min(16) {
            let c = acpi.cpus[i];
            let line = alloc::format!(
                "    MADT cpu acpi_id={} apic_id={} enabled={} online_cap={}\n",
                c.acpi_processor_id, c.apic_id, c.enabled, c.online_capable
            );
            console.write(&line, WHITE);
        }
    }
}

pub fn cmd_ioreg(console: &mut Console<'_>) {
    let entries = crate::drivers::iokit::snapshot_ioregistry();
    let st = crate::drivers::iokit::stats();
    console.write("\nXNU-Style IOKit Device Registry (IORegistry):\n", GREEN);
    let hdr = alloc::format!(
        "  IOAPIC Routed Vectors: {}  |  Total Hardware IRQs: {}  |  WorkLoops: {}\n  Signals: knet_worker={}  kiokit_audio={}  kiokit_blk={}\n\n",
        st.routed_vectors,
        st.total_ioapic_irqs,
        if st.workloops_active { "ARMED" } else { "INACTIVE" },
        st.net_signals,
        st.audio_signals,
        st.blk_signals,
    );
    console.write(&hdr, GRAY);

    for e in &entries {
        let mut indent = alloc::string::String::new();
        for _ in 0..e.depth {
            indent.push_str("  ");
        }
        let color = match e.class_name {
            "IOPlatformExpert" | "IOACPIPlatformDevice" => GREEN,
            "IOInterruptEventSource" | "IOWorkLoop" => YELLOW,
            _ => WHITE,
        };
        let irq_tag = match (e.gsi, e.vector) {
            (Some(gsi), Some(vec)) => alloc::format!(" [GSI {} -> IDT 0x{:02x}, {} irqs]", gsi, vec, e.irq_count),
            (Some(gsi), None) => alloc::format!(" [GSI base {}]", gsi),
            _ => alloc::string::String::new(),
        };
        let wl_tag = if !e.workloop.is_empty() && e.class_name != "IOWorkLoop" {
            alloc::format!(" -> {}", e.workloop)
        } else {
            alloc::string::String::new()
        };
        let line = alloc::format!(
            "{}+-o {}  <class {}, {}>  {}{}{}\n",
            indent,
            e.name,
            e.class_name,
            e.status,
            e.bus_info,
            irq_tag,
            wl_tag,
        );
        console.write(&line, color);
    }
}

pub fn cmd_klog(console: &mut Console<'_>, arguments: &str) {
    let arg = arguments.trim();
    if arg == "clear" {
        crate::klog::clear();
        console.write("Kernel log ring buffer cleared.\n", GREEN);
        return;
    }

    let st = crate::klog::stats();
    if arg == "stats" {
        let line = alloc::format!(
            "\nKernel Log & Telemetry:\n  Events: {} total ({} buffered, {} dropped IRQ)\n  APIC Timer IRQs: {}  |  KThread Preempts: {}  |  User Preempts: {}\n  Lazy #PF Commits: {}  |  Guard Faults: {}  |  User #PF Kills: {}  |  copy_*_user Faults: {}\n",
            st.total_events,
            st.buffered,
            st.dropped_irq_logs,
            st.apic_timer_irqs,
            st.kthread_preempts,
            st.user_preempts,
            st.pf_lazy_commits,
            st.pf_guard_faults,
            st.pf_user_kills,
            st.copy_user_faults,
        );
        console.write(&line, WHITE);
        return;
    }

    let entries = crate::klog::snapshot();
    console.write(
        &alloc::format!(
            "\nKernel Log Ring Buffer ({}/{} entries, total events={}):\n",
            entries.len(),
            crate::klog::KLOG_CAPACITY,
            st.total_events
        ),
        GREEN,
    );
    if entries.is_empty() {
        console.write("  <empty>\n", GRAY);
    } else {
        for e in &entries {
            let color = match e.level {
                crate::klog::KLogLevel::Fault => RED,
                crate::klog::KLogLevel::Warn => YELLOW,
                crate::klog::KLogLevel::Preempt => GREEN,
                crate::klog::KLogLevel::Mm => WHITE,
                crate::klog::KLogLevel::Info => GRAY,
            };
            let line = alloc::format!(
                "  #{:<4} [{:>7}ms] {:<7} [{}] {}\n",
                e.seq,
                e.uptime_ms,
                e.level.label(),
                e.tag,
                e.msg
            );
            console.write(&line, color);
        }
    }
    let summary = alloc::format!(
        "Telemetry: apic_irqs={} kthread_preempts={} user_preempts={} lazy_pf={} guard_pf={} copy_faults={}\n",
        st.apic_timer_irqs,
        st.kthread_preempts,
        st.user_preempts,
        st.pf_lazy_commits,
        st.pf_guard_faults,
        st.copy_user_faults,
    );
    console.write(&summary, GRAY);
}

pub fn cmd_kthread(console: &mut Console<'_>, arguments: &str) {
    let arg = arguments.trim();
    if arg == "demo" {
        let (a, b) = crate::task::kthread::spawn_preempt_demo_pair();
        console.write(
            &alloc::format!(
                "Spawned preemptive kernel demo workers: {:?}, {:?}\n",
                a, b
            ),
            GREEN,
        );
        if !crate::graphics::wm::is_desktop_active() {
            let ran = crate::task::kthread::step_workers(8, 25);
            console.write(
                &alloc::format!("Stepped {} preemptive kthread slice(s).\n", ran),
                GRAY,
            );
        }
    }

    let guarded = crate::task::kthread::are_guarded_stacks_active();
    let preempts = crate::task::kthread::total_preempt_count();
    let list = crate::task::kthread::snapshot_kthreads();
    console.write(
        &alloc::format!(
            "\nKernel Threads (kthread) — Guarded Stacks: {}  |  Total IRQ Preempts: {}\n",
            if guarded { "ACTIVE (4K guard page)" } else { "FALLBACK" },
            preempts
        ),
        GREEN,
    );
    console.write("  ID  CPU  NAME                 STATE      CPU_MS  SWITCHES  PREEMPTS  STACK_BASE\n", GRAY);
    for kt in &list {
        let cpu_tag = match kt.running_on_cpu {
            Some(c) => alloc::format!("*{}", c),
            None => alloc::format!("C{}", kt.last_cpu),
        };
        let line = alloc::format!(
            "  K{:<2} {:<3}  {:<20} {:<10} {:<7} {:<9} {:<9} 0x{:x}{}\n",
            kt.id,
            cpu_tag,
            kt.name,
            kt.state_label,
            kt.cpu_ms,
            kt.switches,
            kt.preempts,
            kt.stack_base,
            if kt.guarded_stack { " [G]" } else { "" },
        );
        console.write(&line, WHITE);
    }
    console.write("Tip: 'kthread demo' spawns a CPU-bound preemptive kthread pair across SMP cores.\n", GRAY);
}

pub fn cmd_ahci(console: &mut Console<'_>, arguments: &str) {
    let snap = crate::drivers::ahci::snapshot();
    console.write("\nAHCI SATA DMA Controller:\n", GREEN);
    if !snap.present {
        console.write("  Not detected on PCI (add QEMU -device ahci,id=ahci).\n", YELLOW);
        return;
    }
    let port_str = match snap.active_port {
        Some(p) => alloc::format!("Port {}", p),
        None => alloc::format!("None"),
    };
    let line = alloc::format!(
        "  ABAR phys=0x{:x} virt=0x{:x} vs=0x{:08x} PI=0x{:x} slots={}\n  Active port: {}  Model: {}  Sectors: {} ({} MiB)\n  DMA Reads: {}  Writes: {}  MaxDMA: {} sectors  Batches: {}\n",
        snap.abar_phys,
        snap.abar_virt,
        snap.version,
        snap.ports_implemented,
        snap.port_slots,
        port_str,
        snap.model_str(),
        snap.sector_count,
        (snap.sector_count * 512) / (1024 * 1024),
        snap.reads,
        snap.writes,
        snap.max_dma_sectors,
        snap.dma_batches,
    );
    console.write(&line, WHITE);

    let trimmed = arguments.trim();
    if let Some(rest) = trimmed.strip_prefix("read ") {
        if let Ok(lba) = rest.trim().parse::<u64>() {
            let mut sec = [0u8; 512];
            match crate::drivers::ahci::read_sector_dma(lba, &mut sec) {
                Ok(()) => {
                    console.write("  AHCI DMA Read OK (first 32 bytes): ", GREEN);
                    for b in &sec[..32] {
                        console.write(&alloc::format!("{:02x} ", b), WHITE);
                    }
                    console.write("\n", WHITE);
                }
                Err(e) => {
                    console.write(&alloc::format!("  AHCI DMA Read Error: {:?}\n", e), RED);
                }
            }
        }
    }
}

pub fn cmd_virtio(console: &mut Console<'_>, arguments: &str) {
    let snap = crate::drivers::virtio_blk::snapshot();
    console.write("\nVirtIO-Blk PCI Driver:\n", GREEN);
    if !snap.present {
        console.write("  Not detected on PCI (use QEMU -drive file=disk.img,if=virtio).\n", YELLOW);
        return;
    }
    let line = alloc::format!(
        "  Initialized: {}  Mode: {}  io_base=0x{:x} mmio=0x{:x}\n  QueueSize: {}  Sectors: {} ({} MiB)  Features: 0x{:08x}\n  DMA Reads: {}  Writes: {}  MaxDMA: {} sectors  Batches: {}\n",
        snap.initialized,
        if snap.is_modern { "Modern" } else { "Legacy/Transitional" },
        snap.io_base,
        snap.mmio_phys,
        snap.queue_size,
        snap.sector_count,
        (snap.sector_count * 512) / (1024 * 1024),
        snap.features,
        snap.reads,
        snap.writes,
        snap.max_dma_sectors,
        snap.dma_batches,
    );
    console.write(&line, WHITE);

    let trimmed = arguments.trim();
    if let Some(rest) = trimmed.strip_prefix("read ") {
        if let Ok(lba) = rest.trim().parse::<u64>() {
            let mut sec = [0u8; 512];
            match crate::drivers::virtio_blk::read_sector_dma(lba, &mut sec) {
                Ok(()) => {
                    console.write("  VirtIO DMA Read OK (first 32 bytes): ", GREEN);
                    for b in &sec[..32] {
                        console.write(&alloc::format!("{:02x} ", b), WHITE);
                    }
                    console.write("\n", WHITE);
                }
                Err(e) => {
                    console.write(&alloc::format!("  VirtIO DMA Read Error: {:?}\n", e), RED);
                }
            }
        }
    }
}

pub fn cmd_ipc(console: &mut Console<'_>) {
    let snap = crate::userland::ipc::snapshot_ipc();
    let percpu = crate::arch::percpu::bsp_snapshot();
    console.write("\nKernel IPC (POSIX Pipes & Shared Memory) + SWAPGS:\n", GREEN);
    let head = alloc::format!(
        "  Active Pipes: {} ({} bytes buffered)\n  SHM Regions:  {} ({} KiB total)\n  Mach Ports:   {} ({} queued msgs, {} bootstrap services)\n  SWAPGS Base:  KERNEL_GS_BASE=0x{:x} BSP_APIC_ID={}\n",
        snap.active_pipes,
        snap.buffered_pipe_bytes,
        snap.shm_regions.len(),
        snap.total_shm_bytes / 1024,
        snap.mach_ports,
        snap.mach_queued_msgs,
        snap.bootstrap_services,
        crate::arch::percpu::read_msr(crate::arch::percpu::IA32_KERNEL_GS_BASE_MSR),
        percpu.apic_id,
    );
    console.write(&head, WHITE);
    for reg in &snap.shm_regions {
        let name = if reg.name.is_empty() { "<anon>" } else { &reg.name };
        let line = alloc::format!(
            "    SHM id={} name='{}' size={}B pages={} refs={}\n",
            reg.id, name, reg.size_bytes, reg.pages, reg.ref_count
        );
        console.write(&line, GRAY);
    }
}

pub fn cmd_nvme(console: &mut Console<'_>, arguments: &str) {
    let snap = crate::drivers::nvme::snapshot();
    console.write("\nNVMe PCI Express Storage (FreeBSD Driver KPI):\n", GREEN);
    if !snap.present {
        console.write(
            "  Not detected on PCI (use QEMU -drive file=disk.img,if=none,id=nvm -device nvme,serial=amaryllis,drive=nvm).\n",
            YELLOW,
        );
        return;
    }

    let irq_str = match (snap.irq_gsi, snap.irq_vector) {
        (Some(gsi), Some(vec)) => alloc::format!("IRQ{} -> GSI {} / IDT 0x{:02x}", snap.irq_line, gsi, vec),
        _ => alloc::format!("IRQ{} (unrouted)", snap.irq_line),
    };
    let line = alloc::format!(
        "  Initialized: {}  PCI {:02}:{:02}.{} ({:04X}:{:04X})  {}\n  BAR0 phys=0x{:x} virt=0x{:x}  VS=0x{:08x}  MQES={} DSTRD={}B\n  Model: {}  Serial: {}  NSID={}  LBA={}B  512B_Sectors={} ({} MiB)\n  DMA Reads: {}  Writes: {}  Batches: {}  MaxDMA: {} sectors  IRQs: {}\n",
        snap.initialized,
        snap.bus,
        snap.device,
        snap.function,
        snap.vendor_id,
        snap.device_id,
        irq_str,
        snap.bar0_phys,
        snap.bar0_virt,
        snap.version,
        snap.max_queue_entries,
        snap.dstrd_bytes,
        snap.model_str(),
        snap.serial_str(),
        snap.nsid,
        snap.lba_size,
        snap.sector_count_512,
        (snap.sector_count_512 * 512) / (1024 * 1024),
        snap.reads,
        snap.writes,
        snap.dma_batches,
        snap.max_dma_sectors,
        crate::drivers::nvme::irq_count(),
    );
    console.write(&line, WHITE);

    let trimmed = arguments.trim();
    if let Some(rest) = trimmed.strip_prefix("read ") {
        if let Ok(lba) = rest.trim().parse::<u64>() {
            let mut sec = [0u8; 512];
            match crate::drivers::nvme::read_sector_dma(lba, &mut sec) {
                Ok(()) => {
                    console.write("  NVMe DMA Read OK (first 32 bytes): ", GREEN);
                    for b in &sec[..32] {
                        console.write(&alloc::format!("{:02x} ", b), WHITE);
                    }
                    console.write("\n", WHITE);
                }
                Err(e) => {
                    console.write(&alloc::format!("  NVMe DMA Read Error: {:?}\n", e), RED);
                }
            }
        }
    }
}

pub fn cmd_kpi(console: &mut Console<'_>) {
    console.write("\nFreeBSD Driver KPI Subsystem:\n", GREEN);

    let devs = crate::freebsd_kpi::snapshot_devices();
    console.write("  Newbus Devices:\n", YELLOW);
    if devs.is_empty() {
        console.write("    (no Newbus KPI devices attached; attach NVMe in QEMU to exercise full stack)\n", GRAY);
    } else {
        for d in &devs {
            let line = alloc::format!(
                "    dev#{} {}{} [{}] drv={} PCI {:02}:{:02}.{} {:04x}:{:04x} BAR0=0x{:x} ({})\n",
                d.id,
                d.name,
                d.unit,
                d.state.as_str(),
                if d.driver_name.is_empty() { "-" } else { d.driver_name },
                d.bus,
                d.device,
                d.function,
                d.vendor_id,
                d.device_id,
                d.bar0_phys,
                d.desc_str(),
            );
            console.write(&line, WHITE);
        }
    }

    let routes = crate::freebsd_kpi::pci_res::snapshot_intr_routes();
    console.write("  KPI IOAPIC IRQ Routes:\n", YELLOW);
    if routes.is_empty() {
        console.write("    (none)\n", GRAY);
    } else {
        for r in &routes {
            let line = alloc::format!(
                "    slot#{} {} IRQ{} -> GSI {} IDT 0x{:02x} calls={}\n",
                r.slot, r.name, r.irq_line, r.gsi, r.vector, r.calls
            );
            console.write(&line, WHITE);
        }
    }

    let (allocs, active_pages, sync_ops, tags) =
        crate::freebsd_kpi::bus_dma::snapshot_dma_stats();
    let dma_line = alloc::format!(
        "  bus_dma: allocs={} active_pages={} sync_ops={} taskq_exec={}\n",
        allocs,
        active_pages,
        sync_ops,
        crate::freebsd_kpi::KPI_TASKQUEUE.executed_count(),
    );
    console.write(&dma_line, WHITE);
    for t in &tags {
        let line = alloc::format!(
            "    tag '{}' align={} maxsize={} segs={} pages={}\n",
            t.name, t.alignment, t.maxsize, t.nsegments, t.allocated_pages
        );
        console.write(&line, GRAY);
    }

    let report = crate::freebsd_kpi::validation_report();
    console.write("  Legacy Reference Validation (e1000 / AHCI):\n", YELLOW);
    for entry in &report.entries {
        console.write(
            if entry.passed { "    [PASS] " } else { "    [FAIL] " },
            if entry.passed { GREEN } else { RED },
        );
        console.write(entry.target, WHITE);
        console.write(": ", GRAY);
        console.write(&entry.summary, GRAY);
        console.write("\n", WHITE);
    }
}

