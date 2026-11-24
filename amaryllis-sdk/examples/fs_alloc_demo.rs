#![no_std]
#![no_main]

use amaryllis_sdk::fs::{self, OpenOptions, SeekFrom};
use amaryllis_sdk::{
    entry, env, format, heap_stats, println, proc, vec, BTreeMap, Box, String, Vec,
};

entry!(main);

fn main() -> i32 {
    println!("=== AmaryllisOS Ring 3 Alloc + VFS + Args Demo (PID={}) ===", proc::getpid());

    let argv: Vec<String> = env::args_vec();
    println!("argc = {} argv = {:?}", argv.len(), argv);

    // 1. Dynamic heap allocation via SYS_BRK (Vec, String, Box, BTreeMap)
    let mut scores: BTreeMap<String, usize> = BTreeMap::new();
    for i in 0..8usize {
        scores.insert(format!("worker-{}", i), i * i + 10);
    }
    let boxed = Box::new([0xAAu8; 4096]);
    println!("BTreeMap entries={}, boxed len={}", scores.len(), boxed.len());

    // 2. Large allocation (>= 64 KiB) backed by SYS_MMAP / SYS_MUNMAP
    let big_buf: Vec<u8> = vec![0x5Au8; 96 * 1024];
    let (brk_b, mmap_b, free_b) = heap_stats();
    println!(
        "Heap before drop: brk={}B mmap={}B free={}B (big[0]=0x{:02x})",
        brk_b, mmap_b, free_b, big_buf[0]
    );
    drop(big_buf);

    // 3. VFS directory creation, write, seek, read, and readdir
    let _ = fs::create_dir("/tmp");
    let test_path = "/tmp/sdk_test.txt";
    let payload = b"Hello AmaryllisOS Ring 3 VFS + Alloc!\nLine 2.\n";
    if let Err(e) = fs::write(test_path, payload) {
        println!("fs::write({}) failed: errno={}", test_path, e);
        return 1;
    }

    match OpenOptions::new().read(true).open(test_path) {
        Ok(mut f) => {
            if let Ok(meta) = f.metadata() {
                println!("Stat {}: size={} dir={}", test_path, meta.len(), meta.is_dir());
            }
            if let Ok(pos) = f.seek(SeekFrom::Start(6)) {
                let mut slice = [0u8; 11];
                let n = f.read(&mut slice).unwrap_or(0);
                let txt = core::str::from_utf8(&slice[..n]).unwrap_or("?");
                println!("Seek({}): read {}B -> {:?}", pos, n, txt);
            }
        }
        Err(e) => {
            println!("Open {} failed: errno={}", test_path, e);
            return 2;
        }
    }

    if let Ok(full) = fs::read_to_string(test_path) {
        println!("Full content ({} chars): {}", full.len(), full.trim());
    }

    println!("Listing '/':");
    if let Ok(entries) = fs::read_dir("/") {
        for item in entries.flatten() {
            let kind = if item.metadata.is_dir() { "DIR " } else { "FILE" };
            println!("  {}  {:>8} B  /{}", kind, item.metadata.len(), item.name);
        }
    }

    println!("Demo complete!");
    0
}
