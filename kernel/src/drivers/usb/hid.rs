use crate::input;

/// USB HID Boot Protocol Keyboard & Mouse decoder feeding AmaryllisOS input subsystem.

pub fn process_keyboard_report(report: &[u8]) {
    if report.len() < 8 {
        return;
    }
    let modifiers = report[0];
    let keys = [
        report[2], report[3], report[4], report[5], report[6], report[7],
    ];
    input::push_hid_key_report(modifiers, &keys);
}

pub fn process_mouse_report(report: &[u8]) {
    if report.len() < 3 {
        return;
    }
    let buttons = report[0] & 0x07;
    let dx = (report[1] as i8) as i16;
    let dy = (report[2] as i8) as i16;
    let wheel = if report.len() >= 4 {
        report[3] as i8
    } else {
        0
    };
    input::mouse::push_usb_packet(dx, dy, wheel, buttons);
}
