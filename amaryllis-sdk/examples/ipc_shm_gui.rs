#![no_std]
#![no_main]

use amaryllis_sdk::gui::{draw_circle_filled, rgb, GuiEvent, ShmWindow};
use amaryllis_sdk::ipc::{pipe, SharedMem};
use amaryllis_sdk::{entry, println, time};

entry!(main);

fn main() -> i32 {
    println!("[ipc_shm_gui] Testing POSIX pipe + named SHM + FPU/SSE fxsave + zero-copy ShmWindow...");

    if let Ok((rx, tx)) = pipe() {
        let _ = tx.write(b"Amaryllis IPC OK");
        let mut msg = [0u8; 32];
        if let Ok(n) = rx.read(&mut msg) {
            if let Ok(text) = core::str::from_utf8(&msg[..n]) {
                println!("[ipc_shm_gui] pipe read: {}", text);
            }
        }
    }

    if let Ok(mut shm) = SharedMem::create_or_open("sdk_demo_shm", 4096, true) {
        if let Some(buf) = shm.as_bytes_mut() {
            buf[0] = 0xAA;
            buf[1] = 0x55;
        }
        println!("[ipc_shm_gui] SHM id={} size={} mapped!", shm.id(), shm.size());
    }

    let Ok(mut win) = ShmWindow::create("Zero-Copy SHM + FPU Demo", 360, 240) else {
        println!("[ipc_shm_gui] Failed to open ShmWindow (Desktop not active?)");
        return 1;
    };

    let mut angle: f64 = 0.25;
    for frame in 0..300u32 {
        match win.poll_event(16) {
            GuiEvent::CloseRequested => break,
            GuiEvent::MouseDown { .. } => angle += 0.5,
            _ => {}
        }

        // Exercise x87/SSE registers across preemptive Ring 3 quanta (fxsave64/fxrstor64)
        angle += 0.085_f64;
        let pulse = ((angle * 1.7_f64) - ((angle * 1.7_f64) as i64 as f64)).abs();
        let w = win.width();
        let h = win.height();
        let pixels = win.pixels_mut();

        for y in 0..h {
            let row = y * w;
            let gy = ((y * 180) / h.max(1)) as u8;
            for x in 0..w {
                let gx = (((x as u32 + frame * 3) * 200) / w.max(1) as u32) as u8;
                pixels[row + x] = rgb(gx / 3, 28 + gy / 2, (95.0 + pulse * 130.0) as u8);
            }
        }

        let cx = ((w as f64) * (0.2 + 0.6 * pulse)) as i32;
        let cy = (h as i32) / 2;
        draw_circle_filled(pixels, w, cx, cy, 26, rgb(75, 227, 166));
        let _ = win.commit();
        let _ = time::sleep_ms(16);
    }

    println!("[ipc_shm_gui] Clean exit.");
    0
}
