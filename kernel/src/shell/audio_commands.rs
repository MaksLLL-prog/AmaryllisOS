use crate::audio;
use crate::console::Console;
use crate::drivers::ac97;
use crate::graphics::Color;

const WHITE: Color = (235, 235, 240);
const GREEN: Color = (40, 180, 120);
const GRAY: Color = (140, 140, 150);
const YELLOW: Color = (220, 190, 90);
const RED: Color = (220, 90, 90);

pub fn cmd_soundinfo(console: &mut Console<'_>, arguments: &str) {
    let trimmed = arguments.trim();
    if let Some(vol_arg) = trimmed.strip_prefix("vol ") {
        if let Ok(pct) = vol_arg.trim().parse::<u8>() {
            match audio::set_volume(pct) {
                Ok(()) => {
                    console.write("Master volume set to ", GREEN);
                    write_decimal(console, pct.min(100) as u64);
                    console.write("%\n", GREEN);
                }
                Err(err) => {
                    console.write("Failed to set volume: ", RED);
                    console.write(err, WHITE);
                    console.write("\n", WHITE);
                }
            }
            return;
        }
    }

    let st = ac97::stats();
    console.write("\nAudio Subsystem Information:\n", GREEN);
    console.write("  Driver:        ", GRAY);
    console.write("Intel AC'97 (ICH 8086:2415)\n", WHITE);

    console.write("  Status:        ", GRAY);
    if !st.initialized {
        console.write("NOT PRESENT (start QEMU with -device AC97)\n", YELLOW);
        return;
    }
    console.write("ACTIVE\n", GREEN);

    console.write("  NAMBAR I/O:    ", GRAY);
    write_hex16(console, st.nambar);
    console.write("\n", WHITE);

    console.write("  NABMBAR I/O:   ", GRAY);
    write_hex16(console, st.nabmbar);
    console.write("\n", WHITE);

    console.write("  IRQ Line:      ", GRAY);
    write_decimal(console, st.irq_line as u64);
    console.write("\n", WHITE);

    console.write("  VRA Support:   ", GRAY);
    console.write(
        if st.vra_supported { "YES\n" } else { "NO (fixed 48000 Hz)\n" },
        WHITE,
    );

    console.write("  Sample Rate:   ", GRAY);
    write_decimal(console, st.sample_rate as u64);
    console.write(" Hz\n", WHITE);

    console.write("  Master Volume: ", GRAY);
    write_decimal(console, st.volume_percent as u64);
    console.write("% (use 'soundinfo vol <0..100>')\n", WHITE);

    console.write("  DMA Buffers:   ", GRAY);
    write_decimal(console, st.buffers_played);
    console.write(" queued\n", WHITE);
}

pub fn cmd_beep(console: &mut Console<'_>, arguments: &str) {
    let mut parts = arguments.split_whitespace();
    let freq = parts
        .next()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(880);
    let dur = parts
        .next()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(220);

    console.write("Beep ", GREEN);
    write_decimal(console, freq as u64);
    console.write(" Hz (", WHITE);
    write_decimal(console, dur as u64);
    console.write(" ms)... ", WHITE);

    match audio::play_tone(freq, dur) {
        Ok(true) => console.write("[AC'97]\n", GREEN),
        Ok(false) => console.write("[PC Speaker fallback]\n", YELLOW),
        Err(err) => {
            console.write("error: ", RED);
            console.write(err, WHITE);
            console.write("\n", WHITE);
        }
    }
}

pub fn cmd_play(console: &mut Console<'_>, arguments: &str) {
    let path = arguments.trim();
    if path.is_empty() {
        console.write("Usage: play <file.wav>\n", YELLOW);
        return;
    }

    console.write("Playing WAV: ", GREEN);
    console.write(path, WHITE);
    console.write(" (press 'q' or Esc to stop)\n", GRAY);

    match audio::wav::play_wav_file(path) {
        Ok((info, cancelled)) => {
            console.write("  Format: PCM ", GRAY);
            write_decimal(console, info.sample_rate as u64);
            console.write(" Hz, ", GRAY);
            write_decimal(console, info.bits_per_sample as u64);
            console.write("-bit, ", GRAY);
            console.write(if info.channels == 2 { "stereo" } else { "mono" }, WHITE);
            console.write(", ", GRAY);
            write_decimal(console, info.duration_ms as u64);
            console.write(" ms\n", GRAY);
            if cancelled {
                console.write("Playback stopped by user.\n", YELLOW);
            } else {
                console.write("Playback completed.\n", GREEN);
            }
        }
        Err(err) => {
            console.write("play error: ", RED);
            console.write(err.message(), WHITE);
            console.write("\n", WHITE);
        }
    }
}

fn write_decimal(console: &mut Console<'_>, mut number: u64) {
    if number == 0 {
        console.write("0", WHITE);
        return;
    }
    let mut buffer = [0u8; 20];
    let mut index = buffer.len();
    while number > 0 {
        index -= 1;
        buffer[index] = b'0' + (number % 10) as u8;
        number /= 10;
    }
    if let Ok(text) = core::str::from_utf8(&buffer[index..]) {
        console.write(text, WHITE);
    }
}

fn write_hex16(console: &mut Console<'_>, value: u16) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    console.write("0x", GRAY);
    let mut buffer = [0u8; 4];
    for index in 0..4 {
        let shift = (3 - index) * 4;
        buffer[index] = HEX[((value >> shift) & 0xF) as usize];
    }
    if let Ok(text) = core::str::from_utf8(&buffer) {
        console.write(text, WHITE);
    }
}
