
use crate::console::Console;
use crate::graphics::Color;
use crate::interrupts::timer;
use crate::task::{Task, TaskState, TaskPriority, SCHEDULER};

use core::sync::atomic::{AtomicU64, Ordering};

use super::parser;

const WHITE: Color = (235, 235, 240);
const GREEN: Color = (40, 180, 120);
const GRAY: Color = (140, 140, 150);
const YELLOW: Color = (220, 190, 90);
const RED: Color = (220, 90, 90);

static TASK_COUNTER_1: AtomicU64 = AtomicU64::new(0);
static TASK_COUNTER_2: AtomicU64 = AtomicU64::new(0);
static TASK_COUNTER_3: AtomicU64 = AtomicU64::new(0);

extern "C" fn demo_task_1() -> ! {
    loop {
        TASK_COUNTER_1.fetch_add(1, Ordering::Relaxed);
        crate::task::sleep_ticks(100);
    }
}

extern "C" fn demo_task_2() -> ! {
    loop {
        TASK_COUNTER_2.fetch_add(1, Ordering::Relaxed);
        crate::task::yield_now();
    }
}

extern "C" fn demo_task_3() -> ! {
    loop {
        TASK_COUNTER_3.fetch_add(1, Ordering::Relaxed);
        crate::task::sleep_ticks(200);
    }
}

// ============================================================
// COMMAND EXECUTION
// ============================================================

pub fn execute(
    console: &mut Console<'_>,
    input: &str,
    mapping: Option<&crate::memory::vmm::PhysicalMemoryMapping>,
    allocator: &mut crate::memory::PhysicalFrameAllocator<'_>,
) {
    let parsed = match parser::parse(input) {
        Some(command) => command,
        None => return,
    };

    let name = parsed.name;
    let arguments = parsed.arguments;

    match name {
        "help" => {
            console.write("Available commands:\n", YELLOW);
            console.write("  help       - Show commands\n", WHITE);
            console.write("  clear      - Clear screen\n", WHITE);
            console.write("  about      - About AmaryllisOS\n", WHITE);
            console.write("  version    - System version\n", WHITE);
            console.write("  echo       - Print text\n", WHITE);
            console.write("  whoami     - Current user\n", WHITE);
            console.write("  neofetch   - System information\n", WHITE);
            console.write("  uptime     - Show system uptime\n", WHITE);
            console.write("  heap       - Heap memory diagnostics\n", WHITE);
            console.write("  heap stress - Stress-test heap allocator\n", WHITE);
            console.write("  pmm        - Physical memory diagnostics\n", WHITE);
            console.write("  pmm test   - Test physical frame allocation\n", WHITE);
            console.write("  pmm stress - Stress-test physical memory allocator\n", WHITE);
            console.write("  tasks      - Task manager\n", WHITE);
            console.write("  block ID   - Block a task\n", WHITE);
            console.write("  unblock ID - Resume a task\n", WHITE);
            console.write("  kill ID    - Finish a task\n", WHITE);
            console.write("  sleep ID TICKS - Put a task to sleep\n", WHITE);
            console.write("  processes  - List processes\n", WHITE);
            console.write("  procinfo PID - Process information\n", WHITE);
            console.write("  prockill PID - Terminate process\n", WHITE);
            console.write("  proccreate - Create process\n", WHITE);

            console.write("\nPCI commands:\n", YELLOW);
            console.write("  lspci              - List PCI devices\n", WHITE);
            console.write("  lspci -v           - Detailed PCI information\n", WHITE);
            console.write("  lspci --drivers    - Show driver bindings\n", WHITE);
            console.write("  lspci --count      - Count PCI devices\n", WHITE);
            console.write("  lspci --help       - PCI command help\n", WHITE);

            console.write("\nFilesystem & Disk commands:\n", YELLOW);
            console.write("  ls [path]          - List directory contents\n", WHITE);
            console.write("  cd [path]          - Change working directory\n", WHITE);
            console.write("  pwd                - Print working directory\n", WHITE);
            console.write("  cat <path>         - Read file contents\n", WHITE);
            console.write("  write <path> <txt> - Create or overwrite file\n", WHITE);
            console.write("  mkdir <path>       - Create directory\n", WHITE);
            console.write("  rm <path>          - Remove file or empty directory\n", WHITE);
            console.write("  diskinfo           - Show disks, partitions, Page Cache & VFS\n", WHITE);
            console.write("  mount [target]     - Mount VFS disk (auto|nvme|virtio|ahci|pm|ps|sm|ss)\n", WHITE);
            console.write("  sync               - Flush dirty Block/Page Cache pages to disk\n", WHITE);

            console.write("\nATA commands:\n", YELLOW);
            console.write("  ata devices        - List detected ATA devices\n", WHITE);
            console.write("  ata read <LBA>     - Read disk sector\n", WHITE);
            console.write("  ata help           - Show ATA command help\n", WHITE);

            console.write("\nDrivers, Platform, Network & Audio commands:\n", YELLOW);
            console.write("  drivers [--active] - List unified kernel drivers\n", WHITE);
            console.write("  ioreg              - XNU-style IOKit IORegistry, IOAPIC GSI & WorkLoop tree\n", WHITE);
            console.write("  acpi | apic | smp  - Inspect ACPI tables, LAPIC timer/IOAPIC & SMP cores\n", WHITE);
            console.write("  klog [stats|clear] - Kernel ring log buffer & preempt/#PF telemetry\n", WHITE);
            console.write("  kthread [demo]     - Inspect preemptive kernel threads & guard stacks\n", WHITE);
            console.write("  ahci [read <LBA>]  - Inspect AHCI SATA DMA HBA & read sectors\n", WHITE);
            console.write("  virtio [read LBA]  - Inspect VirtIO-Blk split-ring DMA driver\n", WHITE);
            console.write("  nvme [read <LBA>]  - Inspect FreeBSD-KPI NVMe DMA driver & read sectors\n", WHITE);
            console.write("  kpi                - Show FreeBSD Driver KPI Newbus, bus_dma & e1000/AHCI checks\n", WHITE);
            console.write("  ipc                - Inspect POSIX pipes, SHM, Mach ports & Bootstrap services\n", WHITE);
            console.write("  netinfo            - Show Intel e1000 & IPv4 status\n", WHITE);
            console.write("  dhcp               - Acquire IPv4 config via DHCP\n", WHITE);
            console.write("  ping <host_or_ip>  - Send 4 ICMP echo packets\n", WHITE);
            console.write("  httpget <url>      - HTTP GET request over TCP\n", WHITE);
            console.write("  soundinfo          - Show Intel AC'97 audio status\n", WHITE);
            console.write("  beep [freq] [ms]   - Play synthesized tone\n", WHITE);
            console.write("  play <file.wav>    - Stream WAV file from FAT32\n", WHITE);

            console.write("\nUserland & ELF (Ring 3) commands:\n", YELLOW);
            console.write("  mkelf [all|path]   - Write /hello,/echo,/fault,/gui_demo,/counter,/net_demo,/lazy,/guard,/fork,/mach,/freebsd ELFs\n", WHITE);
            console.write("  elfinfo <path.elf> - Inspect ELF64 headers and PT_LOAD segments\n", WHITE);
            console.write("  exec <path.elf>    - Execute ELF64 in Ring 3 (non-blocking in Desktop GUI)\n", WHITE);
            console.write("  execbg <path.elf>  - Spawn ELF64 asynchronously in preemptive scheduler\n", WHITE);
            console.write("  ps                 - List active Ring 3 scheduled processes\n", WHITE);
            console.write("  killr3 <pid>       - Terminate an active Ring 3 process by PID\n", WHITE);

            console.write("\nAmaryllis Desktop GUI:\n", YELLOW);
            console.write("  gui | startx       - Launch Amaryllis Desktop Environment (1600x900)\n", WHITE);
        }

        "clear" => {
            console.clear();
            console.draw_rect(0, 0, console.width(), 5, GREEN);
        }

"pmm" => {
    show_pmm(console, arguments, allocator);
}

        "uptime" => {
            let seconds = timer::ticks() / 100;

            console.write("System uptime: ", GREEN);
            write_number(console, seconds);
            console.write(" seconds\n", WHITE);
        }

        "pci" => {
    crate::pci::cli::execute(console, arguments);
}

        "ata" => crate::storage::cli::execute(console, arguments),

        "ls" => super::fs_commands::cmd_ls(console, arguments),
        "cd" => super::fs_commands::cmd_cd(console, arguments),
        "pwd" => super::fs_commands::cmd_pwd(console),
        "cat" => super::fs_commands::cmd_cat(console, arguments),
        "write" => super::fs_commands::cmd_write(console, arguments),
        "mkdir" => super::fs_commands::cmd_mkdir(console, arguments),
        "rm" => super::fs_commands::cmd_rm(console, arguments),
        "diskinfo" => super::fs_commands::cmd_diskinfo(console),
        "mount" => super::fs_commands::cmd_mount(console, arguments),
        "sync" => super::fs_commands::cmd_sync(console),

        "drivers" => super::driver_commands::cmd_drivers(console, arguments),
        "ioreg" => super::driver_commands::cmd_ioreg(console),
        "acpi" => super::driver_commands::cmd_acpi(console),
        "apic" | "smp" => super::driver_commands::cmd_apic(console),
        "klog" | "dmesg" => super::driver_commands::cmd_klog(console, arguments),
        "kthread" | "kthreads" => super::driver_commands::cmd_kthread(console, arguments),
        "ahci" => super::driver_commands::cmd_ahci(console, arguments),
        "virtio" => super::driver_commands::cmd_virtio(console, arguments),
        "nvme" => super::driver_commands::cmd_nvme(console, arguments),
        "kpi" => super::driver_commands::cmd_kpi(console),
        "ipc" => super::driver_commands::cmd_ipc(console),
        "netinfo" => super::net_commands::cmd_netinfo(console),
        "dhcp" => super::net_commands::cmd_dhcp(console),
        "ping" => super::net_commands::cmd_ping(console, arguments),
        "httpget" => super::net_commands::cmd_httpget(console, arguments),
        "soundinfo" => super::audio_commands::cmd_soundinfo(console, arguments),
        "beep" => super::audio_commands::cmd_beep(console, arguments),
        "play" => super::audio_commands::cmd_play(console, arguments),

        "mkelf" => super::user_commands::cmd_mkelf(console, arguments),
        "elfinfo" => super::user_commands::cmd_elfinfo(console, arguments),
        "exec" | "run" => {
            super::user_commands::cmd_exec(console, arguments, mapping, allocator);
        }
        "execbg" => {
            super::user_commands::cmd_execbg(console, arguments, mapping, allocator);
        }
        "ps" => {
            super::user_commands::cmd_ps_ring3(console);
        }
        "killr3" => {
            super::user_commands::cmd_kill_ring3(console, arguments, mapping, allocator);
        }
        "gui" | "startx" => {
            super::gui_commands::cmd_gui(console, arguments, mapping, allocator);
        }

"heap" => {
    match arguments.trim() {
        "" => show_heap(console),
        "stress" => test_heap_stress(console),
        _ => {
            console.write("Usage: heap [stress]\n", YELLOW);
        }
    }
}

        "tasks" => {
            show_tasks(console);
        }

        "block" => {
            manage_task(console, arguments, TaskAction::Block);
        }

        "unblock" => {
            manage_task(console, arguments, TaskAction::Unblock);
        }

        "kill" => {
            manage_task(console, arguments, TaskAction::Finish);
        }

        "sleep" => {
            sleep_task(console, arguments);
        }

        "about" => {
            console.write("AmaryllisOS is a bare-metal x86_64 operating system.\n", WHITE);
            console.write("Written in Rust.\n", GRAY);
        }

        "version" => {
            console.write("AmaryllisOS version 0.10.0\n", GREEN);
            console.write("Architecture: x86_64\n", GRAY);
        }

        "processes" => {
            super::process_commands::show_processes(console);
        }

        "procinfo" => {
            super::process_commands::show_process_info(
                console,
                arguments,
            );
        }

        "proccreate" => {
            super::process_commands::create_process(
                console,
                mapping,
                allocator,
            );
        }

        "prockill" => {
            super::process_commands::kill_process(
                console,
                arguments,
                mapping,
                allocator,
            );
        }

        "lspci" => {
        crate::pci::inspect::execute(console, arguments);
}

        "whoami" => {
            console.write("root\n", GREEN);
        }

        "echo" => {
            console.write(arguments, WHITE);
            console.write("\n", WHITE);
        }

        "neofetch" => {
            console.write("\n", WHITE);

            console.write("       /\\        ", GREEN);
            console.write("AmaryllisOS\n", WHITE);

            console.write("      /  \\       ", GREEN);
            console.write("System: AmaryllisOS\n", WHITE);

            console.write("     / /\\ \\      ", GREEN);
            console.write("Version: 0.10.0\n", WHITE);

            console.write("    / ____ \\     ", GREEN);
            console.write("Kernel: Rust\n", WHITE);

            console.write("   /_/    \\_\\    ", GREEN);
            console.write("Architecture: x86_64\n", WHITE);

            console.write("                  ", WHITE);
            console.write("Shell: amsh\n", WHITE);

            console.write("                  ", WHITE);
            console.write("User: root\n", WHITE);

            console.write("                  ", GRAY);
            console.write("Framebuffer: active\n", GRAY);

            console.write("\n", WHITE);
        }

        _ => {
            console.write("Unknown command: ", RED);
            console.write(name, WHITE);

            if !arguments.is_empty() {
                console.write(" ", WHITE);
                console.write(arguments, WHITE);
            }

            console.write("\nType 'help' for available commands.\n", GRAY);
        }
    }
}

pub fn write_prompt(console: &mut Console<'_>) {
    let cwd = crate::storage::vfs::current_working_directory();
    console.write("amaryllis@localhost:", GREEN);
    console.write(&cwd, YELLOW);
    console.write("$ ", GREEN);
}

// ============================================================
// PCI DEVICE MANAGER
// ============================================================

enum PciDisplayMode {
    List,
    Verbose,
    Drivers,
    Count,
}

fn show_pci(
    console: &mut Console<'_>,
    arguments: &str,
) {
    let mode = match arguments.trim() {
        "" => PciDisplayMode::List,
        "-v" => PciDisplayMode::Verbose,
        "--drivers" => PciDisplayMode::Drivers,
        "--count" => PciDisplayMode::Count,
        "--help" => {
            show_pci_help(console);
            return;
        }
        _ => {
            console.write("Unknown lspci option: ", RED);
            console.write(arguments, WHITE);
            console.write("\nUse 'lspci --help'.\n", GRAY);
            return;
        }
    };

    let total = crate::pci::device_manager::count();

    if matches!(mode, PciDisplayMode::Count) {
        console.write("PCI devices: ", GREEN);
        write_number(console, total as u64);
        console.write("\n", WHITE);
        return;
    }

    console.write("\nPCI Device Manager\n", GREEN);
    console.write("------------------------------\n", GRAY);

    console.write("Registered devices: ", GRAY);
    write_number(console, total as u64);
    console.write("\n\n", WHITE);

    if total == 0 {
        console.write("No PCI devices registered.\n", YELLOW);
        return;
    }

    for index in 0..total {
        let Some(binding) =
            crate::pci::driver::get_binding(index)
        else {
            continue;
        };

        console.write("PCI ", GREEN);
        write_number(console, binding.bus as u64);
        console.write(":", GRAY);
        write_number(console, binding.device as u64);
        console.write(".", GRAY);
        write_number(console, binding.function as u64);

        console.write("  ", WHITE);
        write_hex16(console, binding.vendor_id);
        console.write(":", GRAY);
        write_hex16(console, binding.device_id);

        if matches!(mode, PciDisplayMode::List) {
            console.write("  ", WHITE);
            write_binding_status(console, binding.state);
            console.write("\n", WHITE);
            continue;
        }

        console.write("\n", WHITE);

        console.write("  Vendor ID:       ", GRAY);
        write_hex16(console, binding.vendor_id);
        console.write("\n", WHITE);

        console.write("  Device ID:       ", GRAY);
        write_hex16(console, binding.device_id);
        console.write("\n", WHITE);

        console.write("  Driver:          ", GRAY);

        if let Some(name) = binding.driver_name {
            console.write(name, GREEN);
        } else {
            console.write("None", YELLOW);
        }

        console.write("\n", WHITE);

        if matches!(mode, PciDisplayMode::Drivers) {
            console.write("  Driver priority: ", GRAY);
            write_number(console, binding.priority as u64);
            console.write("\n", WHITE);
        }

        console.write("  Binding:         ", GRAY);
        write_binding_status(console, binding.state);
        console.write("\n", WHITE);

        console.write("  Lifecycle:       ", GRAY);
        write_lifecycle(console, binding.lifecycle);
        console.write("\n\n", WHITE);
    }

    console.write("End of PCI device list.\n", GRAY);
}

fn show_pci_help(console: &mut Console<'_>) {
    console.write("\nPCI commands:\n", YELLOW);
    console.write("  lspci                  - List PCI devices\n", WHITE);
    console.write("  lspci -v               - Detailed device information\n", WHITE);
    console.write("  lspci --drivers        - Show driver bindings\n", WHITE);
    console.write("  lspci --count          - Count PCI devices\n", WHITE);
    console.write("  lspci -s BB:DD.F       - Search by PCI address\n", WHITE);
    console.write("  lspci -d VVVV:DDDD     - Search by Vendor/Device ID\n", WHITE);
    console.write("  lspci --help           - Show PCI help\n", WHITE);
    console.write("  pci resources       - List PCI BAR resources\n", WHITE);
    console.write("  pci memory          - List memory resources\n", WHITE);
    console.write("  pci io              - List I/O resources\n", WHITE);
    console.write("  pci find BB:DD.F    - Find device resources\n", WHITE);
    console.write("  pci validate        - Validate PCI resources\n", WHITE);
    console.write("  pci help            - PCI Resource Manager help\n", WHITE);
}

fn write_binding_status(
    console: &mut Console<'_>,
    state: crate::pci::driver::BindingState,
) {
    use crate::pci::driver::BindingState;

    match state {
        BindingState::Matched => {
            console.write("MATCHED", GREEN);
        }
        BindingState::ProbeRejected => {
            console.write("PROBE REJECTED", RED);
        }
        BindingState::NoDriver => {
            console.write("NO DRIVER", YELLOW);
        }
    }
}

fn write_lifecycle(
    console: &mut Console<'_>,
    lifecycle: crate::pci::driver::DriverLifecycle,
) {
    use crate::pci::driver::DriverLifecycle;

    match lifecycle {
        DriverLifecycle::Registered => {
            console.write("REGISTERED", GRAY);
        }
        DriverLifecycle::Matched => {
            console.write("MATCHED", GREEN);
        }
        DriverLifecycle::Initialized => {
            console.write("INITIALIZED", GREEN);
        }
        DriverLifecycle::Failed => {
            console.write("FAILED", RED);
        }

        DriverLifecycle::Initializing => {
    console.write("INITIALIZING", YELLOW);
}
    }
}

// ============================================================
// HEAP DIAGNOSTICS
// ============================================================

fn show_heap(console: &mut Console<'_>) {
    use crate::memory::heap::GLOBAL_ALLOCATOR;

    console.write("\nHeap Diagnostics\n", GREEN);
    console.write("------------------------------\n", WHITE);

    if !GLOBAL_ALLOCATOR.is_initialized() {
        console.write("Heap is not initialized.\n", RED);
        return;
    }

    let total = GLOBAL_ALLOCATOR.total_size();
    let free = GLOBAL_ALLOCATOR.free_bytes();
    let used = total.saturating_sub(free);
    let free_blocks = GLOBAL_ALLOCATOR.free_blocks();
    let largest_block = GLOBAL_ALLOCATOR.largest_free_block();
    let integrity = GLOBAL_ALLOCATOR.check_integrity();

    console.write("Total memory:       ", GRAY);
    write_number(console, total as u64);
    console.write(" bytes\n", WHITE);

    console.write("Used memory:        ", GRAY);
    write_number(console, used as u64);
    console.write(" bytes\n", WHITE);

    console.write("Free memory:        ", GRAY);
    write_number(console, free as u64);
    console.write(" bytes\n", WHITE);

    console.write("Free blocks:        ", GRAY);
    write_number(console, free_blocks as u64);
    console.write("\n", WHITE);

    console.write("Largest free block: ", GRAY);
    write_number(console, largest_block as u64);
    console.write(" bytes\n", WHITE);

    console.write("Integrity:          ", GRAY);

    if integrity {
        console.write("OK\n", GREEN);
    } else {
        console.write("CORRUPTED\n", RED);
    }

    if free_blocks > 1 {
        console.write("Fragmentation:      ", GRAY);
        console.write("Detected\n", YELLOW);
    } else {
        console.write("Fragmentation:      ", GRAY);
        console.write("Low\n", GREEN);
    }

    console.write("\n", WHITE);
}

// ============================================================
// TASK INITIALIZATION
// ============================================================

enum TaskAction {
    Block,
    Unblock,
    Finish,
}

fn initialize_tasks() {
    let mut scheduler = SCHEDULER.lock();

    if scheduler.task_count() == 0 {
        scheduler.add_task(
            Task::with_priority(1, TaskPriority::High),
            demo_task_1,
        );

        scheduler.add_task(
            Task::with_priority(2, TaskPriority::Normal),
            demo_task_2,
        );

        scheduler.add_task(
            Task::with_priority(3, TaskPriority::Low),
            demo_task_3,
        );
    }
}

// ============================================================
// TASK MANAGER
// ============================================================

fn show_tasks(console: &mut Console<'_>) {
    initialize_tasks();

    let (task_count, task_info) = {
        let scheduler = SCHEDULER.lock();

        let mut info = [None; 16];

        for index in 0..16 {
            let task_id = index + 1;

            if let (
                Some(state),
                Some(base_priority),
                Some(effective_priority),
                Some((run_count, total_runtime, last_scheduled, waiting_time)),
            ) = (
                scheduler.get_task_state(task_id),
                scheduler.get_task_priority(task_id),
                scheduler.get_effective_task_priority(task_id),
                scheduler.get_task_statistics(task_id),
            ) {
                info[index] = Some((
                    task_id,
                    state,
                    base_priority,
                    effective_priority,
                    run_count,
                    total_runtime,
                    last_scheduled,
                    waiting_time,
                ));
            }
        }

        (scheduler.task_count(), info)
    };

    console.write("Task Manager\n", GREEN);
    console.write("------------------------------\n", WHITE);

    console.write("Registered tasks: ", WHITE);
    write_number(console, task_count as u64);
    console.write("\n\n", WHITE);

    for item in task_info.iter().flatten() {
        let (
            id,
            state,
            base,
            effective,
            run_count,
            total_runtime,
            last_scheduled,
            waiting_time,
        ) = *item;

        console.write("Task ", GREEN);
        write_number(console, id as u64);
        console.write("\n", WHITE);

        console.write("  State:             ", GRAY);
        write_task_state(console, state);
        console.write("\n", WHITE);

        console.write("  Base priority:     ", GRAY);
        write_task_priority(console, base);
        console.write("\n", WHITE);

        console.write("  Effective priority:", GRAY);
        console.write(" ", WHITE);
        write_task_priority(console, effective);
        console.write("\n", WHITE);

        console.write("  Run count:         ", GRAY);
        write_number(console, run_count);
        console.write("\n", WHITE);

        console.write("  Total runtime:     ", GRAY);
        write_number(console, total_runtime);
        console.write(" ticks\n", WHITE);

        console.write("  Last scheduled:    ", GRAY);
        write_number(console, last_scheduled);
        console.write(" ticks\n", WHITE);

        console.write("  Waiting time:      ", GRAY);
        write_number(console, waiting_time);
        console.write(" ticks\n\n", WHITE);
    }

    console.write("Task counters:\n", YELLOW);

    console.write("  Task 1: ", WHITE);
    write_number(console, TASK_COUNTER_1.load(Ordering::Relaxed));
    console.write("\n", WHITE);

    console.write("  Task 2: ", WHITE);
    write_number(console, TASK_COUNTER_2.load(Ordering::Relaxed));
    console.write("\n", WHITE);

    console.write("  Task 3: ", WHITE);
    write_number(console, TASK_COUNTER_3.load(Ordering::Relaxed));
    console.write("\n", WHITE);
}

fn manage_task(
    console: &mut Console<'_>,
    arguments: &str,
    action: TaskAction,
) {
    initialize_tasks();

    let id = match arguments.trim().parse::<usize>() {
        Ok(id) if id > 0 => id,
        _ => {
            console.write("Usage: command ID\n", YELLOW);
            return;
        }
    };

    let result = {
        let mut scheduler = SCHEDULER.lock();

        match action {
            TaskAction::Block => scheduler.block_task(id),
            TaskAction::Unblock => scheduler.unblock_task(id),
            TaskAction::Finish => scheduler.finish_task(id),
        }
    };

    if result {
        console.write("Task ", GREEN);
        write_number(console, id as u64);

        match action {
            TaskAction::Block => {
                console.write(" blocked.\n", WHITE);
            }
            TaskAction::Unblock => {
                console.write(" resumed.\n", WHITE);
            }
            TaskAction::Finish => {
                console.write(" marked as Finished.\n", WHITE);
            }
        }
    } else {
        console.write("Operation failed for task ", RED);
        write_number(console, id as u64);
        console.write(". Check its ID and state.\n", WHITE);
    }
}

// ============================================================
// SCHEDULER
// ============================================================

pub fn run_scheduler_step() {
    initialize_tasks();

    let current_ticks = timer::ticks();

    let (selected_id, selected_rsp) = {
        let mut scheduler = SCHEDULER.lock();

        let id = scheduler.next_task(current_ticks);

        let rsp = id.and_then(|task_id| {
            scheduler.task_rsp_ptr(task_id)
        });

        if let (Some(task_id), Some(_)) = (id, rsp) {
            scheduler.set_task_state(task_id, TaskState::Running);
            scheduler.record_task_start(task_id, current_ticks);
        }

        (id, rsp)
    };

    if let (Some(id), Some(rsp)) = (selected_id, selected_rsp) {
        crate::task::context::run_task(rsp);

        let elapsed = timer::ticks().saturating_sub(current_ticks);

        let mut scheduler = SCHEDULER.lock();

        scheduler.record_task_runtime(id, elapsed);

        if scheduler.get_task_state(id) == Some(TaskState::Running) {
            scheduler.set_task_state(id, TaskState::Ready);
        }
    }
}

// ============================================================
// TASK SLEEP
// ============================================================

fn sleep_task(console: &mut Console<'_>, arguments: &str) {
    let mut parts = arguments.split_whitespace();

    let id = match parts.next().and_then(|s| s.parse::<usize>().ok()) {
        Some(id) if id > 0 => id,
        _ => {
            console.write("Usage: sleep ID TICKS\n", YELLOW);
            return;
        }
    };

    let duration = match parts.next().and_then(|s| s.parse::<u64>().ok()) {
        Some(ticks) if ticks > 0 => ticks,
        _ => {
            console.write("Usage: sleep ID TICKS\n", YELLOW);
            return;
        }
    };

    if parts.next().is_some() {
        console.write("Usage: sleep ID TICKS\n", YELLOW);
        return;
    }

    let current_ticks = timer::ticks();

    let result = {
        let mut scheduler = SCHEDULER.lock();
        scheduler.sleep_task(id, duration, current_ticks)
    };

    if result {
        console.write("Task ", GREEN);
        write_number(console, id as u64);
        console.write(" sleeping for ", WHITE);
        write_number(console, duration);
        console.write(" ticks.\n", WHITE);
    } else {
        console.write("Cannot put task to sleep. Check ID and state.\n", RED);
    }
}

// ============================================================
// TASK DISPLAY HELPERS
// ============================================================

fn write_task_state(console: &mut Console<'_>, state: TaskState) {
    match state {
        TaskState::Ready => console.write("Ready", GREEN),
        TaskState::Running => console.write("Running", YELLOW),
        TaskState::Blocked => console.write("Blocked", RED),
        TaskState::Sleeping => console.write("Sleeping", YELLOW),
        TaskState::Finished => console.write("Finished", GRAY),
    }
}

fn write_task_priority(
    console: &mut Console<'_>,
    priority: TaskPriority,
) {
    match priority {
        TaskPriority::Low => console.write("Low", GRAY),
        TaskPriority::Normal => console.write("Normal", WHITE),
        TaskPriority::High => console.write("High", GREEN),
    }
}

// ============================================================
// NUMBER OUTPUT
// ============================================================

fn write_number(console: &mut Console<'_>, mut value: u64) {
    if value == 0 {
        console.write("0", WHITE);
        return;
    }

    let mut buffer = [0u8; 20];
    let mut index = buffer.len();

    while value > 0 {
        index -= 1;
        buffer[index] = b'0' + (value % 10) as u8;
        value /= 10;
    }

    if let Ok(text) = core::str::from_utf8(&buffer[index..]) {
        console.write(text, WHITE);
    }
}


// ============================================================
// PHYSICAL MEMORY MANAGER
// ============================================================

fn show_pmm(
    console: &mut Console<'_>,
    arguments: &str,
    allocator: &mut crate::memory::PhysicalFrameAllocator<'_>,
) {
match arguments.trim() {
    "" => show_pmm_stats(console, allocator),
    "test" => test_pmm(console, allocator),
    "stress" => test_pmm_stress(console, allocator),
    _ => {
        console.write("Usage: pmm [test|stress]\n", YELLOW);
    }
}
}

fn show_pmm_stats(
    console: &mut Console<'_>,
    allocator: &crate::memory::PhysicalFrameAllocator<'_>,
) {
    console.write("\nPhysical Memory Manager\n", GREEN);
    console.write("------------------------------\n", GRAY);

    console.write("Backend:             ", GRAY);

    if allocator.is_dynamic() {
        console.write("Dynamic PMM\n", GREEN);
    } else {
        console.write("Bootstrap PMM\n", YELLOW);
    }

    console.write("Allocated frames:   ", GRAY);
    write_number(console, allocator.allocated_frames() as u64);
    console.write("\n", WHITE);

    console.write("Free frames:        ", GRAY);
    write_number(console, allocator.free_frames() as u64);
    console.write("\n", WHITE);

    console.write("Total allocations:  ", GRAY);
    write_number(console, allocator.total_allocations());
    console.write("\n", WHITE);

    console.write("Total deallocations:", GRAY);
    console.write(" ", WHITE);
    write_number(console, allocator.total_deallocations());
    console.write("\n", WHITE);

    console.write("Integrity:          ", GRAY);

    if allocator.check_integrity() {
        console.write("OK\n", GREEN);
    } else {
        console.write("CORRUPTED\n", RED);
    }

    console.write("\n", WHITE);
}

// ============================================================
// PMM ALLOCATION TEST
// ============================================================

fn test_pmm(
    console: &mut Console<'_>,
    allocator: &mut crate::memory::PhysicalFrameAllocator<'_>,
) {
    console.write("\nDynamic PMM allocation test\n", GREEN);
    console.write("------------------------------\n", GRAY);

    if !allocator.is_dynamic() {
        console.write(
            "Test requires Dynamic PMM backend.\n",
            RED,
        );
        return;
    }

    if !allocator.check_integrity() {
        console.write("PMM integrity check failed.\n", RED);
        return;
    }

    let initial_allocated = allocator.allocated_frames();
    let initial_allocations = allocator.total_allocations();
    let initial_deallocations = allocator.total_deallocations();

    // Allocate first frame.
    let first = match allocator.allocate_frame() {
        Some(frame) => frame,
        None => {
            console.write("First allocation failed.\n", RED);
            return;
        }
    };

    console.write("[ OK ] First frame allocated\n", GREEN);

    // Allocate second frame.
    let second = match allocator.allocate_frame() {
        Some(frame) => frame,
        None => {
            allocator.deallocate_frame(first);
            console.write("Second allocation failed.\n", RED);
            return;
        }
    };

    if first == second {
        allocator.deallocate_frame(first);
        allocator.deallocate_frame(second);
        console.write("Duplicate frame detected.\n", RED);
        return;
    }

    console.write("[ OK ] Second unique frame allocated\n", GREEN);

    // Verify ownership.
    let ownership_valid =
        allocator.owns_frame(first)
        && allocator.owns_frame(second);

    if !ownership_valid {
        allocator.deallocate_frame(first);
        allocator.deallocate_frame(second);
        console.write("Frame ownership check failed.\n", RED);
        return;
    }

    console.write("[ OK ] Frame ownership verified\n", GREEN);

    // Release both frames.
    let first_freed = allocator.deallocate_frame(first);
    let second_freed = allocator.deallocate_frame(second);

    if !first_freed || !second_freed {
        console.write("Frame deallocation failed.\n", RED);
        return;
    }

    console.write("[ OK ] Frames deallocated\n", GREEN);

    // Verify released frames are no longer owned.
    if allocator.owns_frame(first) || allocator.owns_frame(second) {
        console.write("Released frame still marked allocated.\n", RED);
        return;
    }

    console.write("[ OK ] Released frames verified\n", GREEN);

    // Allocate again and verify reuse.
    let reused = match allocator.allocate_frame() {
        Some(frame) => frame,
        None => {
            console.write("Frame reuse allocation failed.\n", RED);
            return;
        }
    };

    let reused_expected = reused == first || reused == second;

    if !allocator.deallocate_frame(reused) {
        console.write("Failed to release reused frame.\n", RED);
        return;
    }

    if !reused_expected {
        console.write(
            "[WARN] Allocator returned another free frame.\n",
            YELLOW,
        );
    } else {
        console.write("[ OK ] Released frame reused\n", GREEN);
    }

    // Verify final state.
    let integrity = allocator.check_integrity();
    let allocation_count_valid =
        allocator.allocated_frames() == initial_allocated;

    let counters_valid =
        allocator.total_allocations() == initial_allocations + 3
        && allocator.total_deallocations() == initial_deallocations + 3;

    console.write("Final integrity:    ", GRAY);
    console.write(
        if integrity { "PASS\n" } else { "FAIL\n" },
        if integrity { GREEN } else { RED },
    );

    console.write("Allocation balance: ", GRAY);
    console.write(
        if allocation_count_valid { "PASS\n" } else { "FAIL\n" },
        if allocation_count_valid { GREEN } else { RED },
    );

    console.write("Operation counters: ", GRAY);
    console.write(
        if counters_valid { "PASS\n" } else { "FAIL\n" },
        if counters_valid { GREEN } else { RED },
    );

    let success =
        integrity
        && allocation_count_valid
        && counters_valid;

    console.write(
        if success {
            "\nDynamic PMM test PASSED.\n"
        } else {
            "\nDynamic PMM test FAILED.\n"
        },
        if success { GREEN } else { RED },
    );
}


// ============================================================
// PMM STRESS TEST
// ============================================================

fn test_pmm_stress(
    console: &mut Console<'_>,
    allocator: &mut crate::memory::PhysicalFrameAllocator<'_>,
) {
    const STRESS_FRAMES: usize = 256;
    const STRESS_ROUNDS: u64 = 2;

    console.write("\nDynamic PMM stress test\n", GREEN);
    console.write("------------------------------\n", GRAY);

    if !allocator.is_dynamic() {
        console.write("Stress test requires Dynamic PMM.\n", RED);
        return;
    }

    if !allocator.check_integrity() {
        console.write("Initial PMM integrity check failed.\n", RED);
        return;
    }

    let initial_allocated = allocator.allocated_frames();
    let initial_allocations = allocator.total_allocations();
    let initial_deallocations = allocator.total_deallocations();

    let mut frames = [0u64; STRESS_FRAMES];
    let mut success = true;

    for round in 0..STRESS_ROUNDS {
        console.write("\nRound ", GRAY);
        write_number(console, round + 1);
        console.write(": allocating ", WHITE);
        write_number(console, STRESS_FRAMES as u64);
        console.write(" frames\n", WHITE);

        let mut allocated = 0usize;

        // Allocate frames and check uniqueness.
        while allocated < STRESS_FRAMES {
            let frame = match allocator.allocate_frame() {
                Some(frame) => frame,
                None => {
                    console.write("Allocation failed.\n", RED);
                    success = false;
                    break;
                }
            };

            let mut duplicate = false;

            for existing in frames.iter().take(allocated) {
                if *existing == frame {
                    duplicate = true;
                    break;
                }
            }

            if duplicate {
                console.write("Duplicate physical frame detected.\n", RED);
                success = false;
                break;
            }

            frames[allocated] = frame;
            allocated += 1;
        }

        if success {
            console.write("[ OK ] All frames allocated\n", GREEN);
        }

        // Verify ownership.
        if success {
            let mut ownership_valid = true;

            for frame in frames.iter().take(allocated) {
                if !allocator.owns_frame(*frame) {
                    ownership_valid = false;
                    break;
                }
            }

            if ownership_valid {
                console.write("[ OK ] Frame ownership verified\n", GREEN);
            } else {
                console.write("Frame ownership verification failed.\n", RED);
                success = false;
            }
        }

        // Release every successfully allocated frame.
        let mut deallocation_valid = true;

        for frame in frames.iter().take(allocated) {
            if !allocator.deallocate_frame(*frame) {
                deallocation_valid = false;
            }
        }

        if deallocation_valid && allocated == STRESS_FRAMES {
            console.write("[ OK ] All frames deallocated\n", GREEN);
        } else {
            console.write("Frame deallocation failed.\n", RED);
            success = false;
        }

        // Verify that released frames are no longer allocated.
        let mut release_valid = true;

        for frame in frames.iter().take(allocated) {
            if allocator.owns_frame(*frame) {
                release_valid = false;
                break;
            }
        }

        if release_valid {
            console.write("[ OK ] Released frames verified\n", GREEN);
        } else {
            console.write("Released frames remain allocated.\n", RED);
            success = false;
        }

        if !allocator.check_integrity() {
            console.write("PMM integrity check failed.\n", RED);
            success = false;
        } else {
            console.write("[ OK ] PMM integrity verified\n", GREEN);
        }

        if !success {
            break;
        }
    }

    let expected_operations = STRESS_FRAMES as u64 * STRESS_ROUNDS;

    let allocation_balance =
        allocator.allocated_frames() == initial_allocated;

    let counters_valid =
        allocator.total_allocations()
            == initial_allocations + expected_operations
        && allocator.total_deallocations()
            == initial_deallocations + expected_operations;

    let final_integrity = allocator.check_integrity();

    console.write("\nStress test summary\n", GREEN);

    console.write("Allocated per round: ", GRAY);
    write_number(console, STRESS_FRAMES as u64);
    console.write("\n", WHITE);

    console.write("Completed rounds:    ", GRAY);
    write_number(
        console,
        if success { STRESS_ROUNDS } else { 0 },
    );
    console.write("\n", WHITE);

    console.write("Allocation balance:  ", GRAY);
    console.write(
        if allocation_balance { "PASS\n" } else { "FAIL\n" },
        if allocation_balance { GREEN } else { RED },
    );

    console.write("Operation counters:  ", GRAY);
    console.write(
        if counters_valid { "PASS\n" } else { "FAIL\n" },
        if counters_valid { GREEN } else { RED },
    );

    console.write("Final integrity:     ", GRAY);
    console.write(
        if final_integrity { "PASS\n" } else { "FAIL\n" },
        if final_integrity { GREEN } else { RED },
    );

    success = success
        && allocation_balance
        && counters_valid
        && final_integrity;

    console.write(
        if success {
            "\nDynamic PMM stress test PASSED.\n"
        } else {
            "\nDynamic PMM stress test FAILED.\n"
        },
        if success { GREEN } else { RED },
    );
}


// ============================================================
// HEAP STRESS TEST
// ============================================================

fn test_heap_stress(console: &mut Console<'_>) {
    use crate::memory::heap::GLOBAL_ALLOCATOR;
    use core::alloc::Layout;
    use core::ptr::NonNull;

    const BLOCK_COUNT: usize = 32;

    console.write("\nHeap stress test\n", GREEN);
    console.write("------------------------------\n", GRAY);

    if !GLOBAL_ALLOCATOR.is_initialized() {
        console.write("Heap is not initialized.\n", RED);
        return;
    }

    if !GLOBAL_ALLOCATOR.check_integrity() {
        console.write("Initial heap integrity failed.\n", RED);
        return;
    }

    let initial_free = GLOBAL_ALLOCATOR.free_bytes();
    let initial_blocks = GLOBAL_ALLOCATOR.free_blocks();

    let mut pointers: [Option<NonNull<u8>>; BLOCK_COUNT] =
        [None; BLOCK_COUNT];

    let mut success = true;
    let mut allocated = 0usize;

    // Allocate blocks with different sizes and alignments.
    for index in 0..BLOCK_COUNT {
        let size = 32 + index * 17;

        let alignment = match index % 4 {
            0 => 8,
            1 => 16,
            2 => 32,
            _ => 64,
        };

        let layout = match Layout::from_size_align(size, alignment) {
            Ok(layout) => layout,
            Err(_) => {
                success = false;
                break;
            }
        };

        match GLOBAL_ALLOCATOR.allocate(layout) {
            Some(pointer) => {
                if pointer.as_ptr() as usize % alignment != 0 {
                    console.write("Alignment check failed.\n", RED);
                    success = false;
                    break;
                }

                pointers[index] = Some(pointer);
                allocated += 1;
            }
            None => {
                console.write("Allocation failed.\n", RED);
                success = false;
                break;
            }
        }
    }

    if success && allocated == BLOCK_COUNT {
        console.write("[ OK ] 32 allocations completed\n", GREEN);
        console.write("[ OK ] Alignment verified\n", GREEN);
    }

    // Check heap integrity while blocks are allocated.
    let allocation_integrity = GLOBAL_ALLOCATOR.check_integrity();

    if allocation_integrity {
        console.write("[ OK ] Integrity after allocation\n", GREEN);
    } else {
        console.write("Integrity after allocation failed.\n", RED);
        success = false;
    }

    // Release blocks in reverse order.
    let mut deallocation_valid = true;

    for index in (0..BLOCK_COUNT).rev() {
        if let Some(pointer) = pointers[index].take() {
            if !unsafe { GLOBAL_ALLOCATOR.deallocate(pointer) } {
                deallocation_valid = false;
            }
        }
    }

    if deallocation_valid && allocated == BLOCK_COUNT {
        console.write("[ OK ] All blocks deallocated\n", GREEN);
    } else {
        console.write("Deallocation failed.\n", RED);
        success = false;
    }

    // Verify that adjacent free blocks were merged.
    let final_free = GLOBAL_ALLOCATOR.free_bytes();
    let final_blocks = GLOBAL_ALLOCATOR.free_blocks();
    let final_integrity = GLOBAL_ALLOCATOR.check_integrity();

    let memory_restored = final_free == initial_free;
    let blocks_restored = final_blocks == initial_blocks;

    console.write("[ CHECK ] Memory restored: ", GRAY);
    console.write(
        if memory_restored { "PASS\n" } else { "FAIL\n" },
        if memory_restored { GREEN } else { RED },
    );

    console.write("[ CHECK ] Free blocks restored: ", GRAY);
    console.write(
        if blocks_restored { "PASS\n" } else { "FAIL\n" },
        if blocks_restored { GREEN } else { RED },
    );

    console.write("[ CHECK ] Final integrity: ", GRAY);
    console.write(
        if final_integrity { "PASS\n" } else { "FAIL\n" },
        if final_integrity { GREEN } else { RED },
    );

    success = success
        && allocated == BLOCK_COUNT
        && deallocation_valid
        && memory_restored
        && blocks_restored
        && final_integrity;

    console.write(
        if success {
            "\nHeap stress test PASSED.\n"
        } else {
            "\nHeap stress test FAILED.\n"
        },
        if success { GREEN } else { RED },
    );
}


fn write_hex16(
    console: &mut Console<'_>,
    value: u16,
) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";

    console.write("0x", GRAY);

    let mut buffer = [0u8; 4];

    for index in 0..4 {
        let shift = (3 - index) * 4;
        buffer[index] = HEX[((value >> shift) & 0xF) as usize];
    }

    if let Ok(text) = core::str::from_utf8(&buffer) {
        console.write(text, WHITE);
    }
}
