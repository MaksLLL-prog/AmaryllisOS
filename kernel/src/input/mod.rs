
mod keyboard;
pub mod mouse;

pub use keyboard::{
    decode_scancode,
    handle_interrupt,
    read_scancode,
    scancode_to_char,
    KeyCode,
    KeyEvent,
    KeyModifiers,
};
pub use mouse::MousePacket;

pub fn initialize_mouse() -> bool {
    mouse::initialize()
}

pub fn read_mouse_wheel() -> Option<i8> {
    mouse::read_wheel()
}

pub fn read_mouse_packet() -> Option<MousePacket> {
    mouse::read_packet()
}

pub fn drain_mouse() {
    mouse::drain();
}

pub fn poll_mouse_hw() -> usize {
    mouse::poll_aux_if_pending()
}

pub fn poll_keyboard_hw() -> usize {
    keyboard::poll_if_pending()
}
