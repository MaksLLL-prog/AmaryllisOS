#![no_std]
#![no_main]

use amaryllis_sdk::ipc::{bootstrap_look_up, bootstrap_register, MachOolBuf, MachPort};
use amaryllis_sdk::syscall::{syscall2, syscall3, SYS_MMAP, SYS_MUNMAP};
use amaryllis_sdk::{entry, println, proc, time};

entry!(main);

const SERVICE_NAME: &str = "org.amaryllis.echo";
const RPC_ECHO_OOL: u32 = 1001;
const RPC_ECHO_REPLY: u32 = 2001;

fn main() -> i32 {
    let srv_port = match MachPort::allocate() {
        Ok(p) => p,
        Err(e) => {
            println!("[mach_ipc_demo] port allocate failed: {}", e);
            return 1;
        }
    };
    if let Err(e) = bootstrap_register(SERVICE_NAME, &srv_port) {
        println!("[mach_ipc_demo] bootstrap_register failed: {}", e);
        return 2;
    }

    match proc::fork() {
        Ok(0) => {
            // Child acts as client (receive right was kept private to parent; client looks up service)
            drop(srv_port);
            time::sleep_ms(20);

            let client_port = match bootstrap_look_up(SERVICE_NAME) {
                Ok(p) => p,
                Err(e) => {
                    println!("[client] bootstrap_look_up failed: {}", e);
                    return 3;
                }
            };

            let ool_ptr = unsafe { syscall3(SYS_MMAP, 0, 4096, 3) };
            if ool_ptr < 0 {
                println!("[client] mmap OOL page failed: {}", ool_ptr);
                return 4;
            }
            let ool_slice =
                unsafe { core::slice::from_raw_parts_mut(ool_ptr as u64 as *mut u8, 4096) };
            let payload = b"Zero-Copy OOL payload from child via Mach IPC!";
            ool_slice[..payload.len()].copy_from_slice(payload);

            match client_port.rpc_call(
                RPC_ECHO_OOL,
                b"ping",
                Some(MachOolBuf {
                    data: ool_slice,
                    deallocate: false,
                }),
                128,
                Some(3000),
            ) {
                Ok(reply) => {
                    let msg = core::str::from_utf8(reply.inline_bytes()).unwrap_or("<non-utf8>");
                    println!(
                        "[client PID={}] RPC reply id={}: '{}'",
                        proc::getpid(),
                        reply.header.id,
                        msg
                    );
                }
                Err(e) => {
                    println!("[client] rpc_call failed: {}", e);
                    let _ = unsafe { syscall2(SYS_MUNMAP, ool_ptr as u64, 4096) };
                    return 5;
                }
            }

            let _ = unsafe { syscall2(SYS_MUNMAP, ool_ptr as u64, 4096) };
            0
        }
        Ok(child_pid) => {
            // Parent acts as Bootstrap Mach server
            match srv_port.recv(128, Some(4000)) {
                Ok(mut req) => {
                    let inline_txt = core::str::from_utf8(req.inline_bytes()).unwrap_or("?");
                    let ool_head = req
                        .ool_bytes()
                        .and_then(|b| core::str::from_utf8(&b[..46]).ok())
                        .unwrap_or("<none>");
                    println!(
                        "[server PID={}] recv id={} inline='{}' ool='{}'",
                        proc::getpid(),
                        req.header.id,
                        inline_txt,
                        ool_head
                    );
                    if let Some(reply_port) = req.take_reply_port() {
                        let _ = reply_port.send_reply(RPC_ECHO_REPLY, b"pong-ool-ok", None);
                    }
                }
                Err(e) => {
                    println!("[server] recv failed: {}", e);
                    let _ = proc::waitpid(child_pid as isize);
                    return 6;
                }
            }

            let (reaped, status) = proc::waitpid(child_pid as isize).unwrap_or((0, -1));
            println!(
                "[server PID={}] child {} exited with status {}",
                proc::getpid(),
                reaped,
                status
            );
            status
        }
        Err(e) => {
            println!("[mach_ipc_demo] fork failed: {}", e);
            7
        }
    }
}
