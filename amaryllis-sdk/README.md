# amaryllis-sdk

### Rust SDK for Ring 3 Applications on Amaryllis OS

`amaryllis-sdk` is a standalone `#![no_std]` Rust SDK for developing external applications that run in **Ring 3** on Amaryllis OS.

The SDK provides a familiar Rust development environment with dynamic memory allocation, filesystem access, process management, networking, time utilities, and graphical window support.

Applications are compiled as static **x86_64 ELF64 `ET_EXEC` binaries** and run under the Amaryllis OS preemptive multitasking environment.

## Features

- **Memory allocation** — `Vec`, `String`, `Box`, and `BTreeMap` through the SDK's global allocator.
- **Process management** — process identification, spawning, yielding, and exiting.
- **Filesystem** — file operations, directory enumeration, metadata, and path management.
- **Networking** — TCP, UDP, DNS resolution, ICMP ping, and HTTP GET.
- **GUI** — window creation, pixel-buffer rendering, and event polling.
- **Standard I/O** — formatted output and standard input.
- **Time utilities** — uptime measurement and millisecond sleep.
- **No standard library required** — applications use `#![no_std]` and `#![no_main]`.

## Prerequisites

Install the Rust nightly toolchain and the bare-metal target:

```bash
rustup toolchain install nightly
rustup target add x86_64-unknown-none --toolchain nightly
```

## Building SDK Examples

Run the following commands from `AmaryllisOS/amaryllis-sdk/`:

```bash
cargo +nightly build --example hello --release
cargo +nightly build --example multitask_counter --release
cargo +nightly build --example gui_plasma --release
cargo +nightly build --example net_http_client --release
cargo +nightly build --example fs_alloc_demo --release
```

The compiled ELF64 binaries are located in:

```text
target/x86_64-unknown-none/release/examples/
```

## Running Applications on Amaryllis OS

Copy the compiled binary to a FAT32 disk image accessible by Amaryllis OS. For example:

```text
/disk/hello.elf
```

Applications can then be launched from Amaryllis Terminal:

```bash
exec /disk/hello.elf
```

Additional process commands:

```bash
exec /disk/fs_alloc_demo.elf hello world
exec /disk/gui_plasma.elf
execbg /disk/multitask_counter.elf
ps
killr3 <pid>
```

- `exec` launches an application and waits for it to finish.
- `execbg` launches an application in the background.
- `ps` lists active Ring 3 processes.
- `killr3` terminates a Ring 3 process by PID.

Applications can also be launched by double-clicking supported ELF files in **Files**.

## Creating an External Application

### 1. Create a Cargo project

Create a new binary crate:

```bash
cargo new --bin my-amaryllis-app
cd my-amaryllis-app
```

Add the SDK as a dependency:

```toml
[dependencies]
amaryllis-sdk = { path = "../amaryllis-sdk" }
```

Configure panic behavior and optimization:

```toml
[profile.dev]
panic = "abort"

[profile.release]
panic = "abort"
opt-level = 3
```

### 2. Configure the target and linker

Copy the SDK's `.cargo/config.toml` and `linker.ld` into your application project.

Make sure the linker script is passed to the linker by adding the following arguments to the target's `rustflags`:

```toml
"-C", "link-arg=-Tlinker.ld"
```

The linker script and target configuration must match the SDK's ELF64 executable layout.

### 3. Write your application

Example `src/main.rs`:

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

### 4. Build

```bash
cargo +nightly build --release --target x86_64-unknown-none
```

The resulting executable can be copied to an Amaryllis OS disk image and launched from the system Terminal.

## SDK API Reference

The following table summarizes the syscall-backed interfaces exposed by the SDK.

| Module | Public API | Kernel Interface |
|---|---|---|
| `alloc_heap` | Global allocator, `Vec`, `String`, `Box`, `BTreeMap`, `heap_stats` | `SYS_BRK (12)`, `SYS_MMAP (9)`, `SYS_MUNMAP (11)` |
| `env` | `args`, `args_vec` | SysV `argc` / `argv` initial stack |
| `fs` | `File`, `OpenOptions`, `SeekFrom`, `read`, `read_to_string`, `write`, `read_dir`, `metadata`, `create_dir`, `remove_file` | `SYS_OPEN (2)`, `SYS_CLOSE (3)`, `SYS_STAT (4)`, `SYS_FSTAT (5)`, `SYS_LSEEK (8)`, `SYS_GETDENTS (78)`, `SYS_MKDIR (83)`, `SYS_UNLINK (87)` |
| `io` | `print!`, `println!`, `eprintln!`, `read_stdin` | `SYS_READ (0)`, `SYS_WRITE (1)` |
| `proc` | `getpid`, `yield_now`, `spawn`, `spawn_with_args`, `active_count`, `exit` | `SYS_YIELD (24)`, `SYS_GETPID (39)`, `SYS_EXIT (60)`, `SYS_PROC_COUNT (220)`, `SYS_SPAWN_ELF (310)` |
| `time` | `uptime_ms`, `sleep_ms` | `SYS_SLEEP_MS (35)`, `SYS_UPTIME_MS (96)` |
| `gui` | `Window::create`, `Window::blit`, `Window::poll_event`, `beep` | `SYS_BEEP (200)`, `SYS_CREATE_WINDOW (300)`, `SYS_WINDOW_BLIT (301)`, `SYS_POLL_GUI_EVENT (302)` |
| `net` | `TcpStream`, `UdpSocket`, `ping`, `dns_resolve`, `http_get` | `SYS_CLOSE (3)`, `SYS_SOCKET (41)`, `SYS_CONNECT (42)`, `SYS_SEND (44)`, `SYS_RECV (45)`, `SYS_BIND (49)`, `SYS_NET_PING (210)`, `SYS_DNS_RESOLVE (211)`, `SYS_HTTP_GET (212)` |

## Application Model

Applications built with `amaryllis-sdk` execute in **Ring 3**, separated from kernel execution privilege.

The SDK communicates with the kernel through the Amaryllis OS syscall interface. Memory allocation, process operations, filesystem access, networking, and graphical operations are provided by kernel services.

The SDK is designed for standalone applications rather than kernel modules or Ring 0 components.

## Project Status

`amaryllis-sdk` is part of the Amaryllis OS project. Its API and syscall interface may evolve as the operating system develops.

For the overall system architecture, implementation status, and roadmap, see the [main Amaryllis OS README](../README.md).

## License

Licensed under the Apache License, Version 2.0. See the root `LICENSE` file for details.