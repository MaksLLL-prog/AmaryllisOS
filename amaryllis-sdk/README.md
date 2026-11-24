# `amaryllis-sdk` — Rust Ring 3 SDK for AmaryllisOS

Standalone `#![no_std]` crate for compiling full external **x86_64 static `ET_EXEC` ELF64** applications that run under AmaryllisOS preemptive Ring 3 multitasking, Desktop compositor, and TCP/IP stack.

## Prerequisites

Make sure the bare-metal `x86_64-unknown-none` target is installed on nightly:

```bash
rustup target add x86_64-unknown-none --toolchain nightly
```

## Building Built-in SDK Examples

Run from `AmaryllisOS/amaryllis-sdk/`:

```bash
cargo +nightly build --example hello --release
cargo +nightly build --example multitask_counter --release
cargo +nightly build --example gui_plasma --release
cargo +nightly build --example net_http_client --release
cargo +nightly build --example fs_alloc_demo --release
```

Compiled static ELF64 binaries appear at:

```text
target/x86_64-unknown-none/release/examples/<name>
```

Copy any resulting binary to your AmaryllisOS FAT32 disk image (for example `/disk/app.elf`), then run it inside AmaryllisOS Terminal or double-click it in **Files**:

```text
exec /disk/fs_alloc_demo.elf hello world
exec /disk/gui_plasma.elf
execbg /disk/multitask_counter.elf
ps
killr3 <pid>
```

## Creating Your Own External App

1. Create a new Cargo binary crate depending on `amaryllis-sdk`:

```toml
[package]
name = "my-amaryllis-app"
version = "0.1.0"
edition = "2021"

[dependencies]
amaryllis-sdk = { path = "../amaryllis-sdk" }

[profile.dev]
panic = "abort"

[profile.release]
panic = "abort"
opt-level = 3
```

2. Copy `amaryllis-sdk/.cargo/config.toml` and `amaryllis-sdk/linker.ld` into your crate (adding `"-C", "link-arg=-Tlinker.ld"` in `.cargo/config.toml` or via `build.rs`).
3. Write `src/main.rs` (with full `Vec`, `String`, `BTreeMap`, `fs`, `env::args` support out of the box):

```rust
#![no_std]
#![no_main]

use amaryllis_sdk::{entry, env, fs, println, proc, Vec, String};

entry!(main);

fn main() -> i32 {
    let args: Vec<String> = env::args_vec();
    println!("PID={}, args={:?}", proc::getpid(), args);
    let _ = fs::write("/hello.txt", b"Hello from Ring 3!\n");
    0
}
```

## Supported Syscalls Exposed by `amaryllis-sdk`

| Module | Functions | Kernel Syscalls |
|---|---|---|
| `alloc_heap` | `#[global_allocator]` (`Vec`, `String`, `Box`, `BTreeMap`), `heap_stats` | `SYS_BRK (12)`, `SYS_MMAP (9)`, `SYS_MUNMAP (11)` |
| `env` | `args`, `args_vec` (`SysV` `argc`/`argv` stack setup) | Initial stack / `rdi`, `rsi` |
| `fs` | `File`, `OpenOptions`, `SeekFrom`, `read`, `read_to_string`, `write`, `read_dir`, `metadata`, `create_dir`, `remove_file` | `SYS_OPEN (2)`, `SYS_CLOSE (3)`, `SYS_STAT (4)`, `SYS_FSTAT (5)`, `SYS_LSEEK (8)`, `SYS_GETDENTS (78)`, `SYS_MKDIR (83)`, `SYS_UNLINK (87)` |
| `io` | `print!`, `println!`, `eprintln!`, `read_stdin` | `SYS_READ (0)`, `SYS_WRITE (1)` |
| `proc` | `getpid`, `yield_now`, `spawn`, `spawn_with_args`, `active_count`, `exit` | `SYS_YIELD (24)`, `SYS_GETPID (39)`, `SYS_EXIT (60)`, `SYS_PROC_COUNT (220)`, `SYS_SPAWN_ELF (310)` |
| `time` | `uptime_ms`, `sleep_ms` | `SYS_SLEEP_MS (35)`, `SYS_UPTIME_MS (96)` |
| `gui` | `Window::create`, `Window::blit`, `Window::poll_event`, `beep` | `SYS_BEEP (200)`, `SYS_CREATE_WINDOW (300)`, `SYS_WINDOW_BLIT (301)`, `SYS_POLL_GUI_EVENT (302)` |
| `net` | `TcpStream`, `UdpSocket`, `ping`, `dns_resolve`, `http_get` | `SYS_CLOSE (3)`, `SYS_SOCKET (41)`, `SYS_CONNECT (42)`, `SYS_SEND (44)`, `SYS_RECV (45)`, `SYS_BIND (49)`, `SYS_NET_PING (210)`, `SYS_DNS_RESOLVE (211)`, `SYS_HTTP_GET (212)` |
