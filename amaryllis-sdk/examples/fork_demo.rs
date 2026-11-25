#![no_std]
#![no_main]

use amaryllis_sdk::{
    println,
    proc::{exit, fork, getpid, waitpid},
};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let mut shared_val: u64 = 42;
    println!("[sdk_fork_demo] Parent PID={} initial value={}", getpid(), shared_val);

    match fork() {
        Ok(0) => {
            // Child process: writing to stack triggers Copy-on-Write page split
            shared_val = 99;
            println!("[sdk_fork_demo] Child PID={} mutated value={}", getpid(), shared_val);
            exit(7);
        }
        Ok(child_pid) => {
            if let Ok((reaped_pid, code)) = waitpid(child_pid as isize, 0) {
                println!(
                    "[sdk_fork_demo] Reaped child PID={} exit_code={}, parent isolated value={}",
                    reaped_pid, code, shared_val
                );
            }
            exit(0);
        }
        Err(err) => {
            println!("[sdk_fork_demo] fork() failed: {}", err);
            exit(1);
        }
    }
}
