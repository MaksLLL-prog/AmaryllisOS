use crate::console::Console;
use crate::graphics::Color;
use crate::memory;
use crate::storage::vfs;
use crate::userland::demo;
use crate::userland::elf::{self, PF_R, PF_W, PF_X, PT_LOAD};
use crate::userland::{runner, sched};
use crate::userland::syscall::UserTrapOutcome;

const WHITE: Color = (235, 235, 240);
const GREEN: Color = (40, 180, 120);
const GRAY: Color = (140, 140, 150);
const YELLOW: Color = (220, 190, 90);
const RED: Color = (220, 90, 90);

pub fn cmd_mkelf(console: &mut Console<'_>, arguments: &str) {
    let arg = arguments.trim();

    if arg.is_empty() || arg == "all" {
        let demos: [(&str, fn() -> alloc::vec::Vec<u8>); 11] = [
            ("/hello.elf", demo::build_hello_elf),
            ("/echo.elf", demo::build_echo_elf),
            ("/fault.elf", demo::build_fault_elf),
            ("/gui_demo.elf", demo::build_gui_demo_elf),
            ("/counter.elf", demo::build_counter_elf),
            ("/net_demo.elf", demo::build_net_demo_elf),
            ("/lazy_demo.elf", demo::build_lazy_demo_elf),
            ("/guard_fault.elf", demo::build_guard_fault_elf),
            ("/fork_demo.elf", demo::build_fork_demo_elf),
            ("/mach_ipc_demo.elf", demo::build_mach_ipc_demo_elf),
            ("/freebsd_demo.elf", demo::build_freebsd_demo_elf),
        ];

        for (path, builder) in demos {
            let bytes = builder();
            match vfs::with_vfs(|vfs_state| vfs_state.write_file(path, &bytes)) {
                Ok(()) => {
                    console.write("Created Ring 3 ELF: ", GREEN);
                    console.write(path, WHITE);
                    console.write(" (", GRAY);
                    write_decimal(console, bytes.len() as u64);
                    console.write(" bytes)\n", GRAY);
                }
                Err(err) => {
                    console.write("mkelf ", RED);
                    console.write(path, WHITE);
                    console.write(": ", RED);
                    console.write(err.message(), RED);
                    console.write("\n", WHITE);
                    return;
                }
            }
        }

        console.write(
            "Run 'exec /fork_demo.elf', 'exec /mach_ipc_demo.elf', 'exec /counter.elf', or 'execbg /counter.elf'.\n",
            YELLOW,
        );
        return;
    }

    let (target, builder): (&str, fn() -> alloc::vec::Vec<u8>) = if arg == "echo" {
        ("/echo.elf", demo::build_echo_elf)
    } else if arg == "fault" {
        ("/fault.elf", demo::build_fault_elf)
    } else if arg == "gui" {
        ("/gui_demo.elf", demo::build_gui_demo_elf)
    } else if arg == "counter" {
        ("/counter.elf", demo::build_counter_elf)
    } else if arg == "net" {
        ("/net_demo.elf", demo::build_net_demo_elf)
    } else if arg == "lazy" {
        ("/lazy_demo.elf", demo::build_lazy_demo_elf)
    } else if arg == "guard" {
        ("/guard_fault.elf", demo::build_guard_fault_elf)
    } else if arg == "fork" {
        ("/fork_demo.elf", demo::build_fork_demo_elf)
    } else if arg == "mach" {
        ("/mach_ipc_demo.elf", demo::build_mach_ipc_demo_elf)
    } else if arg == "freebsd" {
        ("/freebsd_demo.elf", demo::build_freebsd_demo_elf)
    } else if arg == "hello" {
        ("/hello.elf", demo::build_hello_elf)
    } else {
        (arg, demo::build_hello_elf)
    };

    let bytes = builder();
    match vfs::with_vfs(|vfs_state| vfs_state.write_file(target, &bytes)) {
        Ok(()) => {
            console.write("Created Ring 3 ELF: ", GREEN);
            console.write(target, WHITE);
            console.write(" (", GRAY);
            write_decimal(console, bytes.len() as u64);
            console.write(" bytes)\n", GRAY);
        }
        Err(err) => {
            console.write("mkelf: ", RED);
            console.write(err.message(), RED);
            console.write("\n", WHITE);
        }
    }
}

pub fn cmd_elfinfo(console: &mut Console<'_>, arguments: &str) {
    let Some(path) = first_arg(arguments) else {
        console.write("Usage: elfinfo <path.elf>\n", YELLOW);
        return;
    };

    match elf::inspect_elf(path) {
        Ok(info) => {
            console.write("\nELF64 Executable Info\n", GREEN);
            console.write("--------------------------------------------------\n", GRAY);
            console.write("File:         ", GRAY);
            console.write(path, WHITE);
            console.write(" (", GRAY);
            write_decimal(console, info.file_size as u64);
            console.write(" bytes)\n", GRAY);

            console.write("Type:         ", GRAY);
            console.write("ET_EXEC (", WHITE);
            write_hex64(console, info.e_type as u64);
            console.write(")\n", WHITE);

            console.write("Machine:      ", GRAY);
            console.write("x86_64 (", WHITE);
            write_hex64(console, info.e_machine as u64);
            console.write(")\n", WHITE);

            console.write("OS ABI:       ", GRAY);
            console.write(info.abi.name(), GREEN);
            console.write("\n", WHITE);

            console.write("Entry RIP:    ", GRAY);
            write_hex64(console, info.entry_point);
            console.write("\n", WHITE);

            console.write("PH entries:   ", GRAY);
            write_decimal(console, info.ph_num as u64);
            console.write("\n\n", WHITE);

            console.write("TYPE     FLAGS  VADDR               FILESZ    MEMSZ\n", GRAY);
            for seg in &info.segments {
                if seg.p_type == PT_LOAD {
                    console.write("PT_LOAD  ", GREEN);
                } else {
                    console.write("OTHER    ", GRAY);
                }

                let mut fbuf = [b'-'; 3];
                if (seg.p_flags & PF_R) != 0 {
                    fbuf[0] = b'R';
                }
                if (seg.p_flags & PF_W) != 0 {
                    fbuf[1] = b'W';
                }
                if (seg.p_flags & PF_X) != 0 {
                    fbuf[2] = b'X';
                }
                if let Ok(flags_str) = core::str::from_utf8(&fbuf) {
                    console.write(flags_str, YELLOW);
                }
                console.write("    ", WHITE);
                write_hex64(console, seg.p_vaddr);
                console.write("  ", WHITE);
                write_decimal(console, seg.p_filesz);
                console.write("       ", WHITE);
                write_decimal(console, seg.p_memsz);
                console.write("\n", WHITE);
            }
            console.write("\n", WHITE);
        }
        Err(err) => {
            console.write("elfinfo: ", RED);
            console.write(err.message(), RED);
            console.write("\n", WHITE);
        }
    }
}

pub fn cmd_execbg(
    console: &mut Console<'_>,
    arguments: &str,
    mapping: Option<&memory::vmm::PhysicalMemoryMapping>,
    allocator: &mut memory::PhysicalFrameAllocator<'_>,
) {
    let argv = split_cmdline_args(arguments);
    let Some(path) = argv.first().copied() else {
        console.write("Usage: execbg <path.elf> [args...]\n", YELLOW);
        return;
    };

    match runner::spawn_elf_async_with_args(path, &argv, mapping, allocator) {
        Ok(pid) => {
            console.write("Spawned Ring 3 process PID ", GREEN);
            write_decimal(console, pid as u64);
            console.write(" (", GRAY);
            console.write(path, WHITE);
            console.write(")\n", GRAY);
            if !crate::graphics::wm::is_desktop_active() {
                if let Some(m) = mapping {
                    let rep = sched::wait_for_pid_sync(console, pid, m, allocator);
                    print_report_outcome(console, path, &rep.pid, rep.entry_point, rep.pages_mapped, rep.bytes_loaded, rep.outcome, true);
                }
            }
        }
        Err(err) => {
            crate::graphics::alerts::report_system_error(
                crate::graphics::alerts::AlertSeverity::Error,
                crate::graphics::alerts::AlertSubsystem::ElfLoader,
                "ELF SPAWN",
                "Failed to spawn ELF64 binary",
                err.message(),
                "Verify path with 'ls', regenerate demos via 'mkelf all', or inspect with 'elfinfo'.",
                crate::graphics::wm::is_desktop_active(),
            );
            console.write("execbg: ", RED);
            console.write(err.message(), RED);
            console.write("\n", WHITE);
        }
    }
}

pub fn cmd_exec(
    console: &mut Console<'_>,
    arguments: &str,
    mapping: Option<&memory::vmm::PhysicalMemoryMapping>,
    allocator: &mut memory::PhysicalFrameAllocator<'_>,
) {
    let argv = split_cmdline_args(arguments);
    let Some(path) = argv.first().copied() else {
        console.write("Usage: exec <path.elf> [args...]\n", YELLOW);
        return;
    };

    // Inside Desktop GUI, run preemptively without blocking compositor or other apps
    if crate::graphics::wm::is_desktop_active() {
        cmd_execbg(console, arguments, mapping, allocator);
        return;
    }

    let initial_allocated = allocator.allocated_frames();

    match runner::load_and_run_elf_with_args(console, path, &argv, mapping, allocator) {
        Ok(report) => {
            let leaked = allocator.allocated_frames() != initial_allocated;
            let ok = allocator.check_integrity() && !leaked;
            print_report_outcome(
                console,
                path,
                &report.pid,
                report.entry_point,
                report.pages_mapped,
                report.bytes_loaded,
                report.outcome,
                ok,
            );
        }
        Err(err) => {
            crate::graphics::alerts::report_system_error(
                crate::graphics::alerts::AlertSeverity::Error,
                crate::graphics::alerts::AlertSubsystem::ElfLoader,
                "ELF EXEC",
                "Failed to load ELF64 binary",
                err.message(),
                "Verify path with 'ls', regenerate demos via 'mkelf all', or inspect with 'elfinfo'.",
                crate::graphics::wm::is_desktop_active(),
            );
            console.write("exec: ", RED);
            console.write(err.message(), RED);
            console.write("\n", WHITE);
        }
    }
}

pub fn cmd_ps_ring3(console: &mut Console<'_>) {
    let list = sched::snapshot_processes();
    if list.is_empty() {
        console.write("No active preemptive Ring 3 processes.\n", GRAY);
        return;
    }
    sched::print_cli_process_table(console);
}

pub fn cmd_kill_ring3(
    console: &mut Console<'_>,
    arguments: &str,
    mapping: Option<&memory::vmm::PhysicalMemoryMapping>,
    allocator: &mut memory::PhysicalFrameAllocator<'_>,
) {
    let Some(arg) = first_arg(arguments) else {
        console.write("Usage: killr3 <pid>\n", YELLOW);
        return;
    };
    let Ok(pid) = arg.parse::<usize>() else {
        console.write("killr3: invalid PID\n", RED);
        return;
    };
    match sched::kill_user_process(pid, mapping, allocator) {
        Ok(()) => {
            console.write("Terminated Ring 3 PID ", GREEN);
            write_decimal(console, pid as u64);
            console.write("\n", WHITE);
        }
        Err(err) => {
            console.write("killr3: ", RED);
            console.write(err, RED);
            console.write("\n", WHITE);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn print_report_outcome(
    console: &mut Console<'_>,
    path: &str,
    pid: &usize,
    entry_point: u64,
    pages_mapped: usize,
    bytes_loaded: u64,
    outcome: UserTrapOutcome,
    ok: bool,
) {
    console.write("\nRing 3 Process Finished\n", GREEN);
    console.write("--------------------------------------------------\n", GRAY);
    console.write("PID:            ", GRAY);
    write_decimal(console, *pid as u64);
    console.write("\n", WHITE);

    console.write("Entry RIP:      ", GRAY);
    write_hex64(console, entry_point);
    console.write("\n", WHITE);

    console.write("User pages:     ", GRAY);
    write_decimal(console, pages_mapped as u64);
    console.write(" (4 KiB frames, ", GRAY);
    write_decimal(console, bytes_loaded);
    console.write(" B loaded)\n", GRAY);

    match outcome {
        UserTrapOutcome::Exited {
            exit_code,
            syscalls_served,
        } => {
            console.write("Syscalls:       ", GRAY);
            write_decimal(console, syscalls_served);
            console.write("\n", WHITE);

            console.write("Exit code:      ", GRAY);
            if exit_code == 0 {
                write_decimal(console, 0);
            } else if exit_code > 0 {
                write_decimal(console, exit_code as u64);
            } else {
                console.write("-", RED);
                write_decimal(console, exit_code.unsigned_abs());
            }
            console.write("\n", WHITE);
        }
        UserTrapOutcome::Faulted {
            fault,
            syscalls_served,
        } => {
            crate::graphics::alerts::report_user_fault(
                *pid,
                path,
                fault,
                syscalls_served,
                ok,
                crate::graphics::wm::is_desktop_active(),
            );
            console.write("Status:         ", GRAY);
            console.write("KILLED BY EXCEPTION (", RED);
            console.write(exception_name(fault.vector), YELLOW);
            console.write(")\n", RED);

            console.write("Fault RIP:      ", GRAY);
            write_hex64(console, fault.rip);
            console.write("\n", WHITE);

            console.write("Fault RSP:      ", GRAY);
            write_hex64(console, fault.rsp);
            console.write("\n", WHITE);

            console.write("Error code:     ", GRAY);
            write_hex64(console, fault.error_code);
            console.write("\n", WHITE);

            if fault.vector == 14 {
                console.write("Fault CR2 addr: ", GRAY);
                write_hex64(console, fault.cr2);
                console.write("\n", WHITE);
            }

            console.write("Syscalls:       ", GRAY);
            write_decimal(console, syscalls_served);
            console.write("\n", WHITE);
        }
    }

    console.write("PMM integrity:  ", GRAY);
    console.write(if ok { "OK\n" } else { "LEAK/ERROR\n" }, if ok { GREEN } else { RED });
}

fn exception_name(vector: u8) -> &'static str {
    match vector {
        0 => "#DE Divide Error",
        1 => "#DB Debug",
        2 => "NMI Interrupt",
        4 => "#OF Overflow",
        5 => "#BR BOUND Range Exceeded",
        6 => "#UD Invalid Opcode",
        7 => "#NM Device Not Available",
        10 => "#TS Invalid TSS",
        11 => "#NP Segment Not Present",
        12 => "#SS Stack Fault",
        13 => "#GP General Protection Fault",
        14 => "#PF Page Fault",
        16 => "#MF x87 FPU Error",
        17 => "#AC Alignment Check",
        18 => "#MC Machine Check",
        19 => "#XM SIMD Floating-Point",
        _ => "CPU Exception",
    }
}

fn first_arg(input: &str) -> Option<&str> {
    let trimmed = input.trim_start();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(rest) = trimmed.strip_prefix('"') {
        if let Some(end) = rest.find('"') {
            return Some(&rest[..end]);
        }
    }
    match trimmed.find(char::is_whitespace) {
        Some(pos) => Some(&trimmed[..pos]),
        None => Some(trimmed),
    }
}

fn split_cmdline_args(input: &str) -> alloc::vec::Vec<&str> {
    let mut out = alloc::vec::Vec::new();
    let mut rem = input.trim_start();
    while !rem.is_empty() && out.len() < 16 {
        if let Some(rest) = rem.strip_prefix('"') {
            if let Some(end) = rest.find('"') {
                out.push(&rest[..end]);
                rem = rest[end + 1..].trim_start();
                continue;
            }
        }
        match rem.find(char::is_whitespace) {
            Some(pos) => {
                out.push(&rem[..pos]);
                rem = rem[pos..].trim_start();
            }
            None => {
                out.push(rem);
                break;
            }
        }
    }
    out
}

fn write_decimal(console: &mut Console<'_>, mut value: u64) {
    if value == 0 {
        console.write("0", WHITE);
        return;
    }

    let mut buf = [0u8; 20];
    let mut idx = buf.len();
    while value > 0 {
        idx -= 1;
        buf[idx] = b'0' + (value % 10) as u8;
        value /= 10;
    }
    if let Ok(s) = core::str::from_utf8(&buf[idx..]) {
        console.write(s, WHITE);
    }
}

fn write_hex64(console: &mut Console<'_>, value: u64) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut buf = [b'0'; 18];
    buf[0] = b'0';
    buf[1] = b'x';
    for i in 0..16 {
        let nibble = ((value >> ((15 - i) * 4)) & 0xF) as usize;
        buf[2 + i] = HEX[nibble];
    }
    if let Ok(s) = core::str::from_utf8(&buf) {
        console.write(s, WHITE);
    }
}
