use core::arch::asm;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RtcDateTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

impl RtcDateTime {
    pub const fn month_short(self) -> &'static str {
        match self.month {
            1 => "Jan",
            2 => "Feb",
            3 => "Mar",
            4 => "Apr",
            5 => "May",
            6 => "Jun",
            7 => "Jul",
            8 => "Aug",
            9 => "Sep",
            10 => "Oct",
            11 => "Nov",
            12 => "Dec",
            _ => "???",
        }
    }

    /// Sakamoto's weekday algorithm (0 = Sun .. 6 = Sat).
    pub const fn weekday_short(self) -> &'static str {
        const T: [i32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
        let m = if self.month < 1 || self.month > 12 {
            1
        } else {
            self.month as i32
        };
        let d = if self.day < 1 || self.day > 31 {
            1
        } else {
            self.day as i32
        };
        let mut y = self.year as i32;
        if m < 3 {
            y -= 1;
        }
        let w = (y + y / 4 - y / 100 + y / 400 + T[(m - 1) as usize] + d).rem_euclid(7);
        match w {
            0 => "Sun",
            1 => "Mon",
            2 => "Tue",
            3 => "Wed",
            4 => "Thu",
            5 => "Fri",
            _ => "Sat",
        }
    }
}

#[inline]
unsafe fn outb(port: u16, val: u8) {
    asm!("out dx, al", in("dx") port, in("al") val, options(nomem, nostack, preserves_flags));
}

#[inline]
unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    asm!("in al, dx", in("dx") port, out("al") val, options(nomem, nostack, preserves_flags));
    val
}

#[inline]
unsafe fn read_cmos(reg: u8) -> u8 {
    outb(0x70, reg | 0x80); // keep NMI disabled during read
    inb(0x71)
}

#[inline]
fn is_updating() -> bool {
    unsafe { (read_cmos(0x0A) & 0x80) != 0 }
}

#[inline]
const fn bcd_to_bin(val: u8) -> u8 {
    (val & 0x0F) + ((val >> 4) * 10)
}

fn read_raw_snapshot() -> (u8, u8, u8, u8, u8, u8, u8) {
    while is_updating() {
        core::hint::spin_loop();
    }
    unsafe {
        let sec = read_cmos(0x00);
        let min = read_cmos(0x02);
        let hour = read_cmos(0x04);
        let day = read_cmos(0x07);
        let month = read_cmos(0x08);
        let year = read_cmos(0x09);
        let reg_b = read_cmos(0x0B);
        (sec, min, hour, day, month, year, reg_b)
    }
}

/// Reads current UTC/BIOS date and time from the x86 CMOS RTC (ports 0x70/0x71).
pub fn read_datetime() -> RtcDateTime {
    let mut snap = read_raw_snapshot();
    for _ in 0..8 {
        let next = read_raw_snapshot();
        if next == snap {
            break;
        }
        snap = next;
    }

    let (mut sec, mut min, mut hour, mut day, mut month, mut year, reg_b) = snap;
    let is_binary = (reg_b & 0x04) != 0;
    let is_24h = (reg_b & 0x02) != 0;

    let pm_flag = (hour & 0x80) != 0;
    if !is_binary {
        sec = bcd_to_bin(sec);
        min = bcd_to_bin(min);
        hour = bcd_to_bin(hour & 0x7F);
        day = bcd_to_bin(day);
        month = bcd_to_bin(month);
        year = bcd_to_bin(year);
    } else {
        hour &= 0x7F;
    }

    if !is_24h {
        if pm_flag && hour < 12 {
            hour += 12;
        } else if !pm_flag && hour == 12 {
            hour = 0;
        }
    }

    RtcDateTime {
        year: 2000u16 + (year as u16),
        month: month.clamp(1, 12),
        day: day.clamp(1, 31),
        hour: hour % 24,
        minute: min % 60,
        second: sec % 60,
    }
}
