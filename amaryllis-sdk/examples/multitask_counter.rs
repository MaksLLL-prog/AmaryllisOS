#![no_std]
#![no_main]

use amaryllis_sdk::{entry, println, proc, time};

entry!(main);

fn main() -> i32 {
    let pid = proc::getpid();
    println!("[worker PID={}] started preemptive multitasking demo", pid);

    for step in 1..=10 {
        let now = time::uptime_ms();
        let active = proc::active_count();
        println!(
            "[worker PID={}] step={}/10 uptime={} ms active_procs={}",
            pid, step, now, active
        );
        time::sleep_ms(300);
        proc::yield_now();
    }

    println!("[worker PID={}] finished cleanly", pid);
    0
}
