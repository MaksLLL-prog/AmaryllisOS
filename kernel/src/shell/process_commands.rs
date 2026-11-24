
use crate::console::Console;
use crate::graphics::Color;
use crate::memory;
use crate::process::{ProcessState, PROCESS_MANAGER};
use crate::task::{TaskPriority, SCHEDULER};

use core::sync::atomic::{AtomicUsize, Ordering};

const WHITE: Color = (235, 235, 240);
const GREEN: Color = (40, 180, 120);
const GRAY: Color = (140, 140, 150);
const YELLOW: Color = (220, 190, 90);
const RED: Color = (220, 90, 90);

static NEXT_PROCESS_TASK_ID: AtomicUsize = AtomicUsize::new(10_000);

// ============================================================
// PROCESS TASK ENTRY
// ============================================================

// Placeholder until user-mode execution (Ring 3) is implemented.
// Process tasks are created in the Blocked state and must not be
// unblocked as executable user processes yet.
extern "C" fn process_placeholder_entry() -> ! {
    loop {
        crate::task::yield_now();
    }
}

// ============================================================
// CREATE PROCESS
// ============================================================

pub fn create_process(
    console: &mut Console<'_>,
    mapping: Option<&memory::vmm::PhysicalMemoryMapping>,
    allocator: &mut memory::PhysicalFrameAllocator<'_>,
) {
    let mapping = match mapping {
        Some(mapping) => mapping,
        None => {
            console.write(
                "Process creation unavailable: physical mapping disabled.\n",
                RED,
            );
            return;
        }
    };

    // Keep the global lock order consistent:
    // ProcessManager -> Scheduler.
    let mut manager = PROCESS_MANAGER.lock();
    let mut scheduler = SCHEDULER.lock();

    if scheduler.task_count() >= 16 {
        console.write(
            "Cannot create process: scheduler capacity reached.\n",
            RED,
        );
        return;
    }

    // Find a task ID that has never been used by the scheduler.
    // Finished tasks currently remain in its task table.
    let mut task_id = None;

    for _ in 0..64 {
        let candidate = NEXT_PROCESS_TASK_ID.fetch_add(
            1,
            Ordering::Relaxed,
        );

        let candidate = candidate.max(1);

        if scheduler.get_task_state(candidate).is_none() {
            task_id = Some(candidate);
            break;
        }
    }

    let task_id = match task_id {
        Some(id) => id,
        None => {
            console.write(
                "Cannot allocate a unique task ID.\n",
                RED,
            );
            return;
        }
    };

    let pid = match manager.create_process_with_task(
        mapping,
        allocator,
        &mut scheduler,
        task_id,
        TaskPriority::Normal,
        process_placeholder_entry,
    ) {
        Ok(pid) => pid,
        Err(_) => {
            console.write(
                "Process creation failed. Check memory and scheduler capacity.\n",
                RED,
            );
            return;
        }
    };

    // Do not let the placeholder kernel task run.
    // It is only a temporary task association for this process.
    if !scheduler.block_task(task_id) {
        // The task was just inserted, so this should never happen.
        // Attempt to roll back the process and its address space.
        let rollback = unsafe {
            manager.terminate_process_with_task(
                pid,
                mapping,
                allocator,
                &mut scheduler,
            )
        };

        if rollback.is_ok() {
            let _ = manager.reap_process(pid);
        }

        console.write(
            "Process creation failed: could not block its task.\n",
            RED,
        );
        return;
    }

    manager.set_process_state(pid, ProcessState::Blocked);

    let process = match manager.get_process(pid) {
        Some(process) => process,
        None => {
            console.write(
                "Process was created, but could not be retrieved.\n",
                RED,
            );
            return;
        }
    };

    console.write("\nProcess created successfully.\n", GREEN);
    console.write("--------------------------------\n", WHITE);

    console.write("PID:             ", GRAY);
    write_number(console, process.pid() as u64);
    console.write("\n", WHITE);

    console.write("Task ID:         ", GRAY);
    write_number(console, task_id as u64);
    console.write("\n", WHITE);

    console.write("Address Space:   ", GRAY);
    write_number(console, process.address_space().id());
    console.write("\n", WHITE);

    console.write("PML4 physical:   ", GRAY);
    write_number(
        console,
        process.address_space().pml4_physical_address(),
    );
    console.write("\n", WHITE);

    console.write("State:           ", GRAY);
    console.write("Blocked\n", YELLOW);

    console.write(
        "\nUser-mode execution is not implemented yet.\n",
        YELLOW,
    );
    console.write("\n", WHITE);
}

// ============================================================
// LIST PROCESSES
// ============================================================

pub fn show_processes(console: &mut Console<'_>) {
    let manager = PROCESS_MANAGER.lock();

    console.write("\nProcess Manager\n", GREEN);
    console.write("--------------------------------\n", WHITE);

    console.write("Registered processes: ", GRAY);
    write_number(console, manager.process_count() as u64);
    console.write("\n\n", WHITE);

    if manager.is_empty() {
        console.write("No registered processes.\n", GRAY);
        return;
    }

    for process in manager.iter() {
        console.write("PID: ", GREEN);
        write_number(console, process.pid() as u64);

        console.write("  State: ", GRAY);
        write_state(console, process.state());

        console.write("  Task: ", GRAY);

        match process.task_id() {
            Some(id) => write_number(console, id as u64),
            None => console.write("-", WHITE),
        }

        console.write("\n", WHITE);
    }

    console.write("\n", WHITE);
}

// ============================================================
// PROCESS INFORMATION
// ============================================================

pub fn show_process_info(
    console: &mut Console<'_>,
    arguments: &str,
) {
    let pid = match parse_pid(arguments) {
        Some(pid) => pid,
        None => {
            console.write("Usage: procinfo PID\n", YELLOW);
            return;
        }
    };

    let manager = PROCESS_MANAGER.lock();

    let process = match manager.get_process(pid) {
        Some(process) => process,
        None => {
            console.write("Process not found: ", RED);
            write_number(console, pid as u64);
            console.write("\n", WHITE);
            return;
        }
    };

    let address_space = process.address_space();

    console.write("\nProcess Information\n", GREEN);
    console.write("--------------------------------\n", WHITE);

    console.write("PID:             ", GRAY);
    write_number(console, process.pid() as u64);
    console.write("\n", WHITE);

    console.write("State:           ", GRAY);
    write_state(console, process.state());
    console.write("\n", WHITE);

    console.write("Task ID:         ", GRAY);

    match process.task_id() {
        Some(id) => write_number(console, id as u64),
        None => console.write("Not attached", YELLOW),
    }

    console.write("\n", WHITE);

    console.write("Address Space ID:", GRAY);
    console.write(" ", WHITE);
    write_number(console, address_space.id());
    console.write("\n", WHITE);

    console.write("PML4 physical:   ", GRAY);
    write_number(
        console,
        address_space.pml4_physical_address(),
    );
    console.write("\n", WHITE);

    console.write("Kernel PML4:     ", GRAY);
    write_number(
        console,
        address_space.kernel_pml4_physical_address(),
    );
    console.write("\n", WHITE);

    console.write("Destroyed:       ", GRAY);

    if address_space.is_destroyed() {
        console.write("Yes\n", RED);
    } else {
        console.write("No\n", GREEN);
    }

    console.write("\n", WHITE);
}

// ============================================================
// TERMINATE PROCESS
// ============================================================

pub fn kill_process(
    console: &mut Console<'_>,
    arguments: &str,
    mapping: Option<&memory::vmm::PhysicalMemoryMapping>,
    allocator: &mut memory::PhysicalFrameAllocator<'_>,
) {
    let pid = match parse_pid(arguments) {
        Some(pid) => pid,
        None => {
            console.write("Usage: prockill PID\n", YELLOW);
            return;
        }
    };

    let mapping = match mapping {
        Some(mapping) => mapping,
        None => {
            console.write(
                "Process termination unavailable: physical mapping disabled.\n",
                RED,
            );
            return;
        }
    };

    let mut manager = PROCESS_MANAGER.lock();

    let (task_id, process_root) = match manager.get_process(pid) {
        Some(process) => (
            process.task_id(),
            process.address_space().pml4_physical_address(),
        ),
        None => {
            console.write("Process not found: ", RED);
            write_number(console, pid as u64);
            console.write("\n", WHITE);
            return;
        }
    };

    // Never destroy the currently active address space.
    let current_root = memory::cr3::current_pml4();

    if process_root == current_root {
        console.write(
            "Termination refused: address space is active.\n",
            RED,
        );
        return;
    }

    // Lock order: ProcessManager -> Scheduler.
    let mut scheduler = SCHEDULER.lock();

    let result = match task_id {
        Some(_) => unsafe {
            manager.terminate_process_with_task(
                pid,
                mapping,
                allocator,
                &mut scheduler,
            )
        },

        None => unsafe {
            manager.terminate_process(
                pid,
                mapping,
                allocator,
            )
        },
    };

    if result.is_err() {
        console.write(
            "Process termination failed. Check process state.\n",
            RED,
        );
        return;
    }

    if manager.reap_process(pid).is_err() {
        console.write(
            "Process terminated, but could not be reaped.\n",
            YELLOW,
        );
        return;
    }

    console.write("Process ", GREEN);
    write_number(console, pid as u64);
    console.write(" terminated and reaped.\n", WHITE);

    console.write("Address space released.\n", GRAY);

    console.write("Frame allocator integrity: ", GRAY);

    if allocator.check_integrity() {
        console.write("OK\n", GREEN);
    } else {
        console.write("FAILED\n", RED);
    }
}

// ============================================================
// HELPERS
// ============================================================

fn parse_pid(arguments: &str) -> Option<usize> {
    let mut parts = arguments.split_whitespace();

    let pid = parts.next()?.parse::<usize>().ok()?;

    if pid == 0 || parts.next().is_some() {
        return None;
    }

    Some(pid)
}

fn write_state(
    console: &mut Console<'_>,
    state: ProcessState,
) {
    match state {
        ProcessState::Created => console.write("Created", GRAY),
        ProcessState::Ready => console.write("Ready", GREEN),
        ProcessState::Running => console.write("Running", YELLOW),
        ProcessState::Blocked => console.write("Blocked", RED),
        ProcessState::Terminated => console.write("Terminated", GRAY),
    }
}

fn write_number(
    console: &mut Console<'_>,
    mut value: u64,
) {
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
