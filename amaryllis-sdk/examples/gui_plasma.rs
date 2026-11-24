#![no_std]
#![no_main]

use amaryllis_sdk::{
    entry,
    gui::{self, GuiEvent, Window},
    println, proc, time,
};

const WIN_W: usize = 360;
const WIN_H: usize = 240;

entry!(main);

fn main() -> i32 {
    let win = match Window::create("SDK Plasma Wave", WIN_W, WIN_H) {
        Ok(w) => w,
        Err(e) => {
            println!("Failed to create window: {}", e);
            return 1;
        }
    };

    let mut fb = [0u32; WIN_W * WIN_H];
    let mut marker_x = (WIN_W / 2) as i32;
    let mut marker_y = (WIN_H / 2) as i32;
    let mut phase = 0u32;

    println!(
        "[gui_plasma PID={}] window id={} opened ({}x{}). Click to ping audio, press 'q' to exit.",
        proc::getpid(),
        win.id(),
        WIN_W,
        WIN_H
    );

    loop {
        match win.poll_event(16) {
            GuiEvent::CloseRequested => break,
            GuiEvent::KeyChar('q') | GuiEvent::KeyChar('Q') => break,
            GuiEvent::MouseDown { x, y, .. } => {
                marker_x = x;
                marker_y = y;
                gui::beep(740, 45);
            }
            _ => {}
        }

        for y in 0..WIN_H {
            let row = y * WIN_W;
            let wy = ((y as u32) + phase) & 255;
            for x in 0..WIN_W {
                let wx = ((x as u32) + (phase >> 1)) & 255;
                let r = ((wx ^ wy) & 0x7F) as u8 + 40;
                let g = (((x as u32).wrapping_add(y as u32).wrapping_add(phase)) & 0x9F) as u8 + 50;
                let b = (140 + ((wx.wrapping_mul(3) + wy) & 0x5F)) as u8;
                fb[row + x] = gui::rgb(r, g, b);
            }
        }

        // Interactive cursor beacon drawn directly from Ring 3 userland
        gui::draw_circle_filled(&mut fb, WIN_W, marker_x, marker_y, 16, gui::rgb(255, 255, 255));
        gui::draw_circle_filled(&mut fb, WIN_W, marker_x, marker_y, 11, gui::rgb(52, 128, 255));

        if win.blit(&fb).is_err() {
            break;
        }

        phase = phase.wrapping_add(2);
        let _ = time::uptime_ms();
    }

    0
}
