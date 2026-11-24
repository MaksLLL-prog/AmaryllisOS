#![no_std]
#![no_main]

use amaryllis_sdk::{entry, println, proc, time};

entry!(main);

fn main() -> i32 {
    println!("Hello from an external Rust Ring 3 ELF on AmaryllisOS!");
    println!(
        "PID = {}, kernel uptime = {} ms, active Ring 3 tasks = {}",
        proc::getpid(),
        time::uptime_ms(),
        proc::active_count()
    );
    0
}
