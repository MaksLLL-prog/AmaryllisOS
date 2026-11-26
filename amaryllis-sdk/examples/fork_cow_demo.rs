#![no_std]
#![no_main]

use amaryllis_sdk::{entry, println, proc, vec};

entry!(main);

fn main() -> i32 {
    let parent_pid = proc::getpid();
    let mut cow_buf = vec![0xAAu8; 4096 * 2];
    cow_buf[0] = 0x11;
    cow_buf[4096] = 0x22;

    println!(
        "[fork_cow_demo PID={}] before fork: buf[0]=0x{:02x}, buf[4096]=0x{:02x}",
        parent_pid, cow_buf[0], cow_buf[4096]
    );

    match proc::fork() {
        Ok(0) => {
            let child_pid = proc::getpid();
            // Trigger CoW page faults only in the child
            cow_buf[0] = 0xCC;
            cow_buf[4096] = 0xDD;
            println!(
                "[fork_cow_demo child PID={}] wrote CoW pages: buf[0]=0x{:02x}, buf[4096]=0x{:02x}",
                child_pid, cow_buf[0], cow_buf[4096]
            );
            42
        }
        Ok(child_pid) => {
            match proc::waitpid(child_pid as isize) {
                Ok((reaped, status)) => {
                    println!(
                        "[fork_cow_demo parent PID={}] reaped PID={} status={}, parent buf[0]=0x{:02x}, buf[4096]=0x{:02x}",
                        parent_pid, reaped, status, cow_buf[0], cow_buf[4096]
                    );
                    if cow_buf[0] == 0x11 && cow_buf[4096] == 0x22 && status == 42 {
                        0
                    } else {
                        1
                    }
                }
                Err(e) => {
                    println!("[fork_cow_demo] waitpid failed: {}", e);
                    2
                }
            }
        }
        Err(e) => {
            println!("[fork_cow_demo] fork failed: {}", e);
            3
        }
    }
}
