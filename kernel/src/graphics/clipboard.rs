use alloc::string::String;
use spin::Mutex;

static SYSTEM_CLIPBOARD: Mutex<String> = Mutex::new(String::new());

pub fn set_text(text: &str) {
    let mut guard = SYSTEM_CLIPBOARD.lock();
    guard.clear();
    guard.push_str(text);
}

pub fn get_text() -> String {
    SYSTEM_CLIPBOARD.lock().clone()
}
