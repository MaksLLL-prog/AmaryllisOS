# amaryllis-sdk

### Native Rust SDK for Amaryllis OS Ring 3 Applications

`amaryllis-sdk` is a standalone Rust SDK for developing native userspace applications for **Amaryllis OS**.

The SDK targets x86_64 and provides a `#![no_std]` execution environment with `alloc`, a global heap allocator, process management, filesystem operations, networking, inter-process communication, time utilities, and graphical window interfaces.

Applications are compiled as static **ELF64 `ET_EXEC` binaries** and execute in Ring 3 under the ANU kernel's preemptive multitasking environment.

The SDK communicates with the kernel through a custom x86_64 system call ABI.

---

## 1. Architecture

The SDK acts as a userspace abstraction layer between native Rust applications and the Amaryllis kernel.

```text
+-------------------------------------------+
|              Ring 3 Application           |
|                                           |
|          Application Logic / Rust         |
+-------------------------------------------+
|               amaryllis-sdk               |
|                                           |
| alloc_heap | env | io | fs | proc         |
| time | net | ipc | gui                    |
+-------------------------------------------+
|             Syscall Interface             |
|                                           |
|             x86_64 SYSCALL                |
+-------------------------------------------+
|                ANU Kernel                 |
|                                           |
| Process Management | Virtual Memory       |
| VFS | Networking | IPC | GUI              |
+-------------------------------------------+
|                  Hardware                 |
+-------------------------------------------+
```

### Execution model

- Applications execute in Ring 3.
- Kernel code executes in Ring 0.
- Applications use independent virtual address spaces.
- Kernel services are accessed through system calls.
- Process scheduling is preemptive.
- Application arguments are passed through the initial `argc` / `argv` stack.
- Executables use the ELF64 `ET_EXEC` format.

The SDK is designed for native Amaryllis OS applications. It is not a Linux compatibility layer and does not provide a general-purpose hosted Rust environment.

---

## 2. Core Features

| Subsystem | Capabilities |
|---|---|
| Memory management | Global allocator, `Vec`, `String`, `Box`, `BTreeMap` |
| Process management | PID, spawning, yielding, process termination |
| Virtual memory | `brk`, `mmap`, `munmap` interfaces |
| Filesystem | File I/O, directory enumeration, metadata, path operations |
| IPC | Kernel-backed pipes and shared memory |
| Networking | TCP, UDP, DNS, ICMP ping, HTTP GET |
| GUI | Window creation, pixel-buffer rendering, event polling |
| Shared graphics | Shared-memory window interfaces |
| Standard I/O | Formatted output and standard input |
| Time | Uptime measurement and millisecond sleep |
| Runtime | `#![no_std]`, `#![no_main]` |

### Memory allocation

The SDK provides dynamic allocation through its global allocator.

Supported collection types include:

- `Vec<T>`
- `String`
- `Box<T>`
- `BTreeMap<K, V>`

The allocator uses kernel virtual memory interfaces, including `brk` and `mmap`-based allocation paths.

### Process management

The `proc` module provides:

- Process identification through `getpid()`.
- Asynchronous ELF application spawning through `spawn()`.
- Command-line argument support through `spawn_with_args()`.
- Voluntary scheduler yielding through `yield_now()`.
- Active Ring 3 process counting through `active_count()`.
- Process termination through `exit()`.

The kernel also implements Copy-on-Write process creation with `fork()` and child-process synchronization with `waitpid()`. However, these interfaces are not yet exposed by the current SDK `proc` module.

### Copy-on-Write memory

The ANU kernel implements Copy-on-Write (CoW) support for process address spaces.

The implementation includes:

- Physical frame reference counting.
- Shared physical pages between address spaces.
- CoW page-fault handling through `#PF`.
- Private page allocation when a shared page is written.

This is a kernel memory-management capability. It should not be confused with a currently available `proc::fork()` SDK function.

### Filesystem

The SDK exposes filesystem operations through the kernel VFS interface:

- File opening and closing.
- Reading and writing.
- Directory enumeration.
- File metadata retrieval.
- Directory creation.
- File removal.
- File seeking.

### Networking

The networking module provides userspace interfaces for:

- TCP streams.
- UDP sockets.
- DNS resolution.
- ICMP ping.
- HTTP GET.

These operations are backed by the Amaryllis kernel networking subsystem.

### Graphical interfaces

The GUI subsystem provides:

- Window creation.
- Pixel-buffer rendering.
- GUI event polling.
- Shared-memory graphical window interfaces.
- Audio beep functionality.

---

## 3. Project Structure

The SDK is maintained as a separate Cargo package inside the Amaryllis OS repository.

```text
amaryllis-sdk/
├── Cargo.toml
├── build.rs
├── .cargo/
│   └── config.toml
├── linker.ld
├── examples/
│   ├── hello.rs
│   ├── multitask_counter.rs
│   ├── gui_plasma.rs
│   ├── net_http_client.rs
│   ├── fs_alloc_demo.rs
│   ├── ipc_shm_gui.rs
│   └── fork_demo.rs
└── src/
    ├── lib.rs
    ├── alloc_heap.rs
    ├── env.rs
    ├── fs.rs
    ├── gui.rs
    ├── io.rs
    ├── ipc.rs
    ├── net.rs
    ├── proc.rs
    ├── syscall.rs
    └── time.rs
```

### Module responsibilities

| Module | Responsibility |
|---|---|
| `alloc_heap` | Global allocator and heap statistics |
| `env` | Application arguments |
| `fs` | Filesystem abstractions |
| `gui` | Window and graphical operations |
| `io` | Standard input and output |
| `ipc` | Pipes and shared memory |
| `net` | Networking interfaces |
| `proc` | Process lifecycle and management |
| `syscall` | Raw system call numbers and ABI wrappers |
| `time` | Uptime and sleep utilities |

---

## 4. Requirements

Install Rust Nightly:

```bash
rustup toolchain install nightly
```

Install the bare-metal target:

```bash
rustup target add x86_64-unknown-none --toolchain nightly
```

Required tools:

- Rust Nightly.
- Cargo.
- `x86_64-unknown-none` target.
- Amaryllis OS source tree for local SDK development.
- QEMU or compatible hardware for execution.

---

## 5. Building SDK Examples

Run the following commands from `AmaryllisOS/amaryllis-sdk/`.

Build individual examples:

```bash
cargo +nightly build --example hello --release
cargo +nightly build --example multitask_counter --release
cargo +nightly build --example gui_plasma --release
cargo +nightly build --example net_http_client --release
cargo +nightly build --example fs_alloc_demo --release
cargo +nightly build --example ipc_shm_gui --release
cargo +nightly build --example fork_demo --release
```

Build all examples:

```bash
cargo +nightly build --examples --release
```

The compiled executables are located in:

```text
target/x86_64-unknown-none/release/examples/
```

### Example applications

| Example | Demonstrates |
|---|---|
| `hello` | Basic Ring 3 application |
| `multitask_counter` | Process execution and scheduling |
| `gui_plasma` | Graphical rendering |
| `net_http_client` | Networking and HTTP |
| `fs_alloc_demo` | Filesystem and dynamic allocation |
| `ipc_shm_gui` | Shared-memory GUI integration |
| `fork_demo` | Fork/CoW-related demonstration; verify SDK integration against the current implementation before relying on it |

---

## 6. Running Applications

Copy the compiled ELF executable to a FAT32 disk image accessible by Amaryllis OS.

Example path:

```text
/disk/hello.elf
```

Launch it from Amaryllis Terminal:

```bash
exec /disk/hello.elf
```

### Process commands

```bash
exec /disk/fs_alloc_demo.elf hello world
exec /disk/gui_plasma.elf
execbg /disk/multitask_counter.elf
ps
killr3 <pid>
```

| Command | Description |
|---|---|
| `exec` | Launch an application and wait for it to finish |
| `execbg` | Launch an application in the background |
| `ps` | List active Ring 3 processes |
| `killr3` | Terminate a Ring 3 process by PID |

Supported ELF applications can also be launched by double-clicking them in Files.

---

## 7. Creating an External Application

### 7.1. Create a Cargo project

```bash
cargo new --bin my-amaryllis-app
cd my-amaryllis-app
```

Add the SDK as a dependency:

```toml
[dependencies]
amaryllis-sdk = { path = "../amaryllis-sdk" }
```

The dependency path must point to the SDK crate directory.

Configure panic behavior and optimization:

```toml
[profile.dev]
panic = "abort"

[profile.release]
panic = "abort"
opt-level = 3
codegen-units = 1
```

### 7.2. Configure the target and linker

Copy the SDK's `.cargo/config.toml` and `linker.ld` into the application project.

The target configuration must select:

```toml
[build]
target = "x86_64-unknown-none"
```

Make sure the linker script is passed to the linker:

```toml
"-C", "link-arg=-Tlinker.ld"
```

Use the SDK's actual target configuration and linker script. The ELF executable layout must match the format expected by the Amaryllis ELF loader.

### 7.3. Write an application

Example `src/main.rs`:

```rust
#![no_std]
#![no_main]

use amaryllis_sdk::{
    entry,
    env,
    fs,
    println,
    proc,
    String,
    Vec,
};

entry!(main);

fn main() -> i32 {
    let args: Vec<String> = env::args_vec();

    println!(
        "PID={}, args={:?}",
        proc::getpid(),
        args
    );

    let _ = fs::write(
        "/hello.txt",
        b"Hello from Ring 3!\n"
    );

    0
}
```

This example demonstrates:

- SDK entry-point initialization.
- Argument retrieval.
- Process identification.
- Formatted output.
- Filesystem access.

### 7.4. Build

```bash
cargo +nightly build --release --target x86_64-unknown-none
```

The resulting ELF executable can be copied to an Amaryllis OS disk image and launched from the system Terminal.

---

## 8. Process API

The `proc` module currently exposes the following public interfaces:

| Function | Description |
|---|---|
| `getpid()` | Returns the calling process PID |
| `yield_now()` | Voluntarily yields the remaining time slice |
| `exit(code)` | Terminates the calling process |
| `spawn(cmdline)` | Spawns an ELF64 application |
| `spawn_with_args(cmdline)` | Alias for `spawn`, accepting a command line |
| `active_count()` | Returns the number of registered Ring 3 processes |

### ELF spawning

`spawn()` accepts a command line containing an executable path and optional arguments.

Example:

```rust
use amaryllis_sdk::proc;

let result = proc::spawn("/disk/worker.elf input.txt");

match result {
    Ok(()) => {
        // Spawn request accepted.
    }
    Err(error) => {
        // Kernel returned an error.
    }
}
```

The implementation passes the command-line pointer and length to `SYS_SPAWN_ELF`.

The function returns `Result<(), i64>`.

---

## 9. Inter-Process Communication

The `ipc` module currently implements kernel-backed pipes and shared memory.

### 9.1. Pipes

The SDK exposes a POSIX-style pipe abstraction:

```rust
use amaryllis_sdk::ipc;

let (reader, writer) = ipc::pipe()?;
```

The pipe consists of two endpoints:

- `PipeReader` — reads data from the pipe.
- `PipeWriter` — writes data into the pipe.

Both endpoints expose `raw_fd()` to retrieve their underlying file descriptor.

Example:

```rust
use amaryllis_sdk::ipc;

let (reader, writer) = ipc::pipe()?;

writer.write(b"Hello through a pipe")?;

let mut buffer = [0u8; 64];
let bytes_read = reader.read(&mut buffer)?;
```

The pipe uses kernel-managed buffering and the regular `read` / `write` syscall interfaces.

Both endpoint types implement `Drop`. Their file descriptors are closed automatically when the objects are dropped.

### 9.2. Shared memory

The SDK provides a `SharedMem` abstraction backed by shared physical frames managed by the kernel.

Create or open a named region:

```rust
use amaryllis_sdk::ipc::SharedMem;

let mut shared = SharedMem::create_or_open(
    "shared-buffer",
    4096,
    true,
)?;
```

Map an existing shared-memory object by numeric ID:

```rust
let shared = SharedMem::map_id(
    id,
    4096,
    false,
)?;
```

Available methods:

| Method | Description |
|---|---|
| `create_or_open(name, size, writable)` | Creates or opens a named SHM region and maps it |
| `map_id(id, size, writable)` | Maps an existing SHM object |
| `id()` | Returns the SHM object ID |
| `size()` | Returns the page-aligned mapping size |
| `as_bytes()` | Returns a read-only byte slice |
| `as_bytes_mut()` | Returns a mutable byte slice when writable |

The requested mapping size is rounded up to a 4 KiB page boundary.

Writable mappings allow mutable access through `as_bytes_mut()`. Read-only mappings return `None` from that method.

`SharedMem` implements `Drop` and automatically unmaps its virtual mapping.

The SDK's shared-memory interface is useful for data exchange between processes and for zero-copy-style graphical data paths.

---

## 10. System Call ABI

The SDK implements raw x86_64 `SYSCALL` wrappers in `syscall.rs`.

The wrappers support zero to four arguments:

- `syscall0`
- `syscall1`
- `syscall2`
- `syscall3`
- `syscall4`

### Register convention

| Register | Purpose |
|---|---|
| `RAX` | System call number and return value |
| `RDI` | Argument 1 |
| `RSI` | Argument 2 |
| `RDX` | Argument 3 |
| `R10` | Argument 4 |
| `RCX` | Clobbered by `SYSCALL` |
| `R11` | Clobbered by `SYSCALL` |

The wrappers use inline assembly and return an `i64`.

Negative return values represent kernel errors. The SDK exposes common negative error constants, including:

- `ENOENT`
- `EIO`
- `EBADF`
- `EAGAIN`
- `ENOMEM`
- `EFAULT`
- `EEXIST`
- `EINVAL`
- `EMFILE`
- `EROFS`
- `EPIPE`
- `ENOSYS`
- `ETIMEDOUT`
- `ECONNREFUSED`

### System call reference

The following numbers are defined in the current SDK `syscall.rs`.

| Subsystem | Constant | Number |
|---|---|---:|
| I/O | `SYS_READ` | 0 |
| I/O | `SYS_WRITE` | 1 |
| Filesystem | `SYS_OPEN` | 2 |
| Filesystem | `SYS_CLOSE` | 3 |
| Filesystem | `SYS_STAT` | 4 |
| Filesystem | `SYS_FSTAT` | 5 |
| Filesystem | `SYS_LSEEK` | 8 |
| Memory | `SYS_MMAP` | 9 |
| Memory | `SYS_MUNMAP` | 11 |
| Memory | `SYS_BRK` | 12 |
| IPC | `SYS_PIPE` | 22 |
| Scheduler | `SYS_YIELD` | 24 |
| Time | `SYS_SLEEP_MS` | 35 |
| Process | `SYS_GETPID` | 39 |
| Networking | `SYS_SOCKET` | 41 |
| Networking | `SYS_CONNECT` | 42 |
| Networking | `SYS_SEND` | 44 |
| Networking | `SYS_RECV` | 45 |
| Networking | `SYS_BIND` | 49 |
| Process | `SYS_EXIT` | 60 |
| Filesystem | `SYS_GETDENTS` | 78 |
| Filesystem | `SYS_MKDIR` | 83 |
| Filesystem | `SYS_UNLINK` | 87 |
| Time | `SYS_UPTIME_MS` | 96 |
| Audio | `SYS_BEEP` | 200 |
| Networking | `SYS_NET_PING` | 210 |
| Networking | `SYS_DNS_RESOLVE` | 211 |
| Networking | `SYS_HTTP_GET` | 212 |
| Process | `SYS_PROC_COUNT` | 220 |
| GUI | `SYS_CREATE_WINDOW` | 300 |
| GUI | `SYS_WINDOW_BLIT` | 301 |
| GUI | `SYS_POLL_GUI_EVENT` | 302 |
| GUI | `SYS_CREATE_SHM_WINDOW` | 303 |
| GUI | `SYS_COMMIT_SHM_WINDOW` | 304 |
| Process | `SYS_SPAWN_ELF` | 310 |
| IPC | `SYS_SHM_CREATE` | 320 |
| IPC | `SYS_SHM_MAP` | 321 |
| IPC | `SYS_SHM_UNMAP` | 322 |

These are the constants defined by the current SDK. They should be kept synchronized with the kernel syscall dispatcher.

---

## 11. API Reference

| Module | Public interfaces | Kernel interfaces |
|---|---|---|
| `alloc_heap` | Global allocator, `Vec`, `String`, `Box`, `BTreeMap`, `heap_stats` | `SYS_BRK`, `SYS_MMAP`, `SYS_MUNMAP` |
| `env` | `args`, `args_vec` | Initial `argc` / `argv` stack |
| `fs` | `File`, `OpenOptions`, `SeekFrom`, `read`, `read_to_string`, `write`, `read_dir`, `metadata`, `create_dir`, `remove_file` | `SYS_OPEN`, `SYS_CLOSE`, `SYS_STAT`, `SYS_FSTAT`, `SYS_LSEEK`, `SYS_GETDENTS`, `SYS_MKDIR`, `SYS_UNLINK` |
| `io` | `print!`, `println!`, `eprintln!`, `read_stdin` | `SYS_READ`, `SYS_WRITE` |
| `proc` | `getpid`, `yield_now`, `spawn`, `spawn_with_args`, `active_count`, `exit` | `SYS_YIELD`, `SYS_GETPID`, `SYS_EXIT`, `SYS_PROC_COUNT`, `SYS_SPAWN_ELF` |
| `time` | `uptime_ms`, `sleep_ms` | `SYS_SLEEP_MS`, `SYS_UPTIME_MS` |
| `gui` | `Window::create`, `Window::blit`, `Window::poll_event`, `beep`, shared-memory window interfaces | `SYS_BEEP`, `SYS_CREATE_WINDOW`, `SYS_WINDOW_BLIT`, `SYS_POLL_GUI_EVENT`, `SYS_CREATE_SHM_WINDOW`, `SYS_COMMIT_SHM_WINDOW` |
| `net` | `TcpStream`, `UdpSocket`, `ping`, `dns_resolve`, `http_get` | `SYS_SOCKET`, `SYS_CONNECT`, `SYS_SEND`, `SYS_RECV`, `SYS_BIND`, `SYS_NET_PING`, `SYS_DNS_RESOLVE`, `SYS_HTTP_GET` |
| `ipc` | `pipe`, `PipeReader`, `PipeWriter`, `SharedMem` | `SYS_PIPE`, `SYS_READ`, `SYS_WRITE`, `SYS_CLOSE`, `SYS_SHM_CREATE`, `SYS_SHM_MAP`, `SYS_SHM_UNMAP` |

---

## 12. ABI and Compatibility

| Property | Value |
|---|---|
| Architecture | x86_64 |
| Binary format | ELF64 |
| ELF type | `ET_EXEC` |
| Execution privilege | Ring 3 |
| Rust environment | `no_std` + `alloc` |
| Entry point | SDK `entry!` macro |
| Argument model | SysV-style `argc` / `argv` |
| System call mechanism | x86_64 `SYSCALL` |
| Kernel | ANU |

Applications must match the executable format, target configuration, linker layout, and syscall conventions supported by the running Amaryllis OS kernel.

The SDK does not claim full POSIX or Linux ABI compatibility.

---

## 13. Current Limitations

Amaryllis OS and its SDK are under active development.

- The SDK targets Amaryllis OS rather than Linux or Windows.
- The SDK uses `no_std` and does not provide the Rust standard library.
- System calls are specific to the Amaryllis kernel.
- Kernel capabilities are not necessarily exposed through public SDK functions.
- Kernel CoW support does not automatically imply an SDK `fork()` wrapper.
- Hardware and driver compatibility depend on the OS implementation.
- Public APIs and syscall conventions may evolve.
- Full POSIX compatibility is not claimed.
- The SDK is intended for userspace applications, not Ring 0 kernel modules.

---

## 14. Project Status

`amaryllis-sdk` is part of the Amaryllis OS v0.11.0 development line.

It provides a native Rust userspace environment with:

- Dynamic memory allocation.
- ELF64 application execution.
- Process management.
- Filesystem access.
- TCP and UDP networking interfaces.
- Kernel-backed pipes.
- Shared-memory mappings.
- Graphical window interfaces.
- Time and standard I/O utilities.

The SDK and kernel are developed together. Their interfaces may change as the ANU kernel evolves.

For the overall system architecture, implementation status, and roadmap, see the [main Amaryllis OS README](../README.md).

---

## License

Licensed under the **Apache License, Version 2.0**.

See the root [`LICENSE`](../LICENSE) file for details.

---

**amaryllis-sdk — Native Rust applications for Amaryllis OS.**