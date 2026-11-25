
use crate::console::Console;
use crate::graphics::Color;
use crate::memory;
use crate::process::ProcessState;
use crate::task::{
    Scheduler,
    TaskPriority,
    TaskState,
    SCHEDULER,
};

const WHITE: Color = (235, 235, 240);
const GREEN: Color = (40, 180, 120);
const GRAY: Color = (140, 140, 150);
const YELLOW: Color = (220, 190, 90);

const TEST_TASK_ID: usize = 9001;

extern "C" fn integration_test_task() -> ! {
    loop {
        core::hint::spin_loop();
    }
}

fn display_result(
    console: &mut Console<'_>,
    name: &str,
    success: bool,
) {
    console.write("  ", GRAY);
    console.write(name, GRAY);
    console.write(": ", GRAY);

    if success {
        console.write("SUCCESS\n", GREEN);
    } else {
        console.write("FAILED\n", YELLOW);
    }
}

#[inline(never)]
pub fn run(
    console: &mut Console<'_>,
    allocator: &mut memory::PhysicalFrameAllocator<'_>,
    mapping: &memory::vmm::PhysicalMemoryMapping,
) {
    console.write(
        "\nProcess and scheduler integration test:\n",
        GREEN,
    );

    let mut manager = crate::process::PROCESS_MANAGER.lock();
    let mut scheduler = SCHEDULER.lock();

    // --------------------------------------------------------
    // 1. Create process and scheduler task
    // --------------------------------------------------------

    let pid = match manager.create_process_with_task(
        mapping,
        allocator,
        &mut scheduler,
        TEST_TASK_ID,
        TaskPriority::Normal,
        integration_test_task,
    ) {
        Ok(pid) => pid,
        Err(_) => {
            display_result(
                console,
                "Process and task creation",
                false,
            );
            return;
        }
    };

    display_result(
        console,
        "Process and task creation",
        true,
    );

    // --------------------------------------------------------
    // 2. Verify PID -> Task ID association
    // --------------------------------------------------------

    let association_valid = manager
        .get_process(pid)
        .map(|process| process.task_id() == Some(TEST_TASK_ID))
        .unwrap_or(false);

    display_result(
        console,
        "PID to Task ID association",
        association_valid,
    );

    // --------------------------------------------------------
    // 3. Verify initial states
    // --------------------------------------------------------

    let process_ready = manager
        .get_process(pid)
        .map(|process| process.state() == ProcessState::Ready)
        .unwrap_or(false);

    let task_ready =
        scheduler.get_task_state(TEST_TASK_ID)
            == Some(TaskState::Ready);

    display_result(
        console,
        "Initial process state",
        process_ready,
    );

    display_result(
        console,
        "Initial task state",
        task_ready,
    );

    // --------------------------------------------------------
    // 4. Synchronize Blocked state
    // --------------------------------------------------------

    let task_blocked = scheduler.set_task_state(
        TEST_TASK_ID,
        TaskState::Blocked,
    );

    let blocked_sync = if task_blocked {
        manager.sync_process_state(pid, &scheduler).is_ok()
    } else {
        false
    };

    let process_blocked = manager
        .get_process(pid)
        .map(|process| process.state() == ProcessState::Blocked)
        .unwrap_or(false);

    display_result(
        console,
        "Task blocked",
        task_blocked,
    );

    display_result(
        console,
        "Blocked state synchronization",
        blocked_sync && process_blocked,
    );

    // --------------------------------------------------------
    // 5. Synchronize Ready state
    // --------------------------------------------------------

    let task_ready_again = scheduler.set_task_state(
        TEST_TASK_ID,
        TaskState::Ready,
    );

    let ready_sync = if task_ready_again {
        manager.sync_process_state(pid, &scheduler).is_ok()
    } else {
        false
    };

    let process_ready_again = manager
        .get_process(pid)
        .map(|process| process.state() == ProcessState::Ready)
        .unwrap_or(false);

    display_result(
        console,
        "Task returned to Ready",
        task_ready_again,
    );

    display_result(
        console,
        "Ready state synchronization",
        ready_sync && process_ready_again,
    );

    // --------------------------------------------------------
    // 6. Verify address space is not active
    // --------------------------------------------------------

    let address_space_root = manager
        .get_process(pid)
        .map(|process| {
            process.address_space().pml4_physical_address()
        });

    let current_root = memory::cr3::current_pml4();

    let address_space_inactive = matches!(
        address_space_root,
        Some(root) if root != current_root
    );

    display_result(
        console,
        "Process address space is inactive",
        address_space_inactive,
    );

    // --------------------------------------------------------
    // 7. Terminate process and task
    // --------------------------------------------------------

    let terminated = if address_space_inactive {
        unsafe {
            manager.terminate_process_with_task(
                pid,
                mapping,
                allocator,
                &mut scheduler,
            ).is_ok()
        }
    } else {
        false
    };

    display_result(
        console,
        "Process and task termination",
        terminated,
    );

    // --------------------------------------------------------
    // 8. Verify terminated states
    // --------------------------------------------------------

    let process_terminated = manager
        .get_process(pid)
        .map(|process| process.state() == ProcessState::Terminated)
        .unwrap_or(false);

    let task_finished =
        scheduler.get_task_state(TEST_TASK_ID)
            == Some(TaskState::Finished);

    display_result(
        console,
        "Process terminated state",
        process_terminated,
    );

    display_result(
        console,
        "Task finished state",
        task_finished,
    );

    // --------------------------------------------------------
    // 9. Reap process
    // --------------------------------------------------------

    let reaped = if terminated {
        manager.reap_process(pid).is_ok()
    } else {
        false
    };

    display_result(
        console,
        "Process reaped",
        reaped,
    );

    display_result(
        console,
        "Process removed from table",
        manager.get_process(pid).is_none(),
    );

    // --------------------------------------------------------
    // 10. Verify allocator integrity
    // --------------------------------------------------------

    display_result(
        console,
        "Physical frame allocator integrity",
        allocator.check_integrity(),
    );
}
