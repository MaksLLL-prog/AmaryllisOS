
use core::arch::asm;
use core::sync::atomic::{AtomicI8, AtomicU64, AtomicU8, AtomicUsize, Ordering};

const DATA_PORT: u16 = 0x60;
const STATUS_PORT: u16 = 0x64;
const COMMAND_PORT: u16 = 0x64;

const QUEUE_SIZE: usize = 32;
const PACKET_QUEUE_SIZE: usize = 64;
const WAIT_LIMIT: usize = 200_000;
const COMMAND_RETRIES: usize = 3;

const STATUS_OUTPUT_FULL: u8 = 1;
const STATUS_INPUT_FULL: u8 = 2;
const STATUS_AUX_DATA: u8 = 0x20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MousePacket {
    pub dx: i16,
    pub dy: i16,
    pub wheel: i8,
    pub buttons: u8,
}

impl MousePacket {
    #[inline]
    pub const fn left(self) -> bool {
        (self.buttons & 0x01) != 0
    }

    #[inline]
    pub const fn right(self) -> bool {
        (self.buttons & 0x02) != 0
    }

    #[inline]
    pub const fn middle(self) -> bool {
        (self.buttons & 0x04) != 0
    }

    #[inline]
    fn pack(self) -> u64 {
        ((self.dx as u16) as u64)
            | (((self.dy as u16) as u64) << 16)
            | (((self.wheel as u8) as u64) << 32)
            | ((self.buttons as u64) << 40)
            | (1u64 << 63)
    }

    #[inline]
    fn unpack(raw: u64) -> Self {
        Self {
            dx: (raw & 0xFFFF) as u16 as i16,
            dy: ((raw >> 16) & 0xFFFF) as u16 as i16,
            wheel: ((raw >> 32) & 0xFF) as u8 as i8,
            buttons: ((raw >> 40) & 0xFF) as u8,
        }
    }
}

static PACKET: [AtomicU8; 4] = [
    AtomicU8::new(0),
    AtomicU8::new(0),
    AtomicU8::new(0),
    AtomicU8::new(0),
];

static PACKET_INDEX: AtomicU8 = AtomicU8::new(0);

static QUEUE: [AtomicI8; QUEUE_SIZE] = [const { AtomicI8::new(0) }; QUEUE_SIZE];
static HEAD: AtomicU8 = AtomicU8::new(0);
static TAIL: AtomicU8 = AtomicU8::new(0);

static PACKET_QUEUE: [AtomicU64; PACKET_QUEUE_SIZE] =
    [const { AtomicU64::new(0) }; PACKET_QUEUE_SIZE];
static PKT_HEAD: AtomicUsize = AtomicUsize::new(0);
static PKT_TAIL: AtomicUsize = AtomicUsize::new(0);
static INITIALIZED: AtomicU8 = AtomicU8::new(0);

unsafe fn inb(port: u16) -> u8 {
    let value: u8;

    asm!(
        "in al, dx",
        in("dx") port,
        out("al") value,
        options(nomem, nostack, preserves_flags)
    );

    value
}

unsafe fn outb(port: u16, value: u8) {
    asm!(
        "out dx, al",
        in("dx") port,
        in("al") value,
        options(nomem, nostack, preserves_flags)
    );
}

fn wait_input_clear() -> bool {
    for _ in 0..WAIT_LIMIT {
        if unsafe { inb(STATUS_PORT) } & STATUS_INPUT_FULL == 0 {
            return true;
        }
    }

    false
}

fn controller_command(command: u8) -> bool {
    if !wait_input_clear() {
        return false;
    }

    unsafe {
        outb(COMMAND_PORT, command);
    }

    true
}

fn write_controller_data(value: u8) -> bool {
    if !wait_input_clear() {
        return false;
    }

    unsafe {
        outb(DATA_PORT, value);
    }

    true
}

// Очищает старые данные из выходного буфера контроллера.
fn flush_output_buffer() {
    for _ in 0..32 {
        let status = unsafe { inb(STATUS_PORT) };

        if status & STATUS_OUTPUT_FULL == 0 {
            break;
        }

        let _ = unsafe { inb(DATA_PORT) };
    }
}

// Читает ответ контроллера, а не мыши.
fn read_controller_data() -> Option<u8> {
    for _ in 0..WAIT_LIMIT {
        let status = unsafe { inb(STATUS_PORT) };

        if status & STATUS_OUTPUT_FULL != 0 && status & STATUS_AUX_DATA == 0 {
            return Some(unsafe { inb(DATA_PORT) });
        }
    }

    None
}

// Читает ответ AUX-устройства.
fn read_aux_data() -> Option<u8> {
    for _ in 0..WAIT_LIMIT {
        let status = unsafe { inb(STATUS_PORT) };

        if status & STATUS_OUTPUT_FULL == 0 {
            continue;
        }

        let value = unsafe { inb(DATA_PORT) };

        if status & STATUS_AUX_DATA != 0 {
            return Some(value);
        }
    }

    None
}

fn mouse_command_once(command: u8) -> Option<u8> {
    if !controller_command(0xD4) {
        return None;
    }

    if !write_controller_data(command) {
        return None;
    }

    read_aux_data()
}

fn mouse_command(command: u8) -> bool {
    for _ in 0..COMMAND_RETRIES {
        match mouse_command_once(command) {
            Some(0xFA) => return true,
            Some(0xFE) => continue,
            _ => return false,
        }
    }

    false
}

fn set_sample_rate(rate: u8) -> bool {
    mouse_command(0xF3) && mouse_command(rate)
}

fn write_configuration(config: u8) -> bool {
    controller_command(0x60) && write_controller_data(config)
}

fn initialize_inner() -> bool {
    INITIALIZED.store(0, Ordering::Release);

    // Сначала отключаем AUX-порт.
    if !controller_command(0xA7) {
        return false;
    }

    // Удаляем старые данные.
    flush_output_buffer();

    // Включаем AUX-порт.
    if !controller_command(0xA8) {
        return false;
    }

    // Читаем конфигурацию i8042.
    if !controller_command(0x20) {
        return false;
    }

    let Some(mut config) = read_controller_data() else {
        return false;
    };

    // Во время настройки IRQ12 выключен.
    // AUX clock включён.
    config &= !(1 << 1);
    config &= !(1 << 5);

    if !write_configuration(config) {
        return false;
    }

    // Сбрасываем настройки мыши к значениям по умолчанию.
    if !mouse_command(0xF6) {
        return false;
    }

    // Включаем режим IntelliMouse.
    if !set_sample_rate(200) || !set_sample_rate(100) || !set_sample_rate(80) {
        return false;
    }

    // Запрашиваем идентификатор.
    if !mouse_command(0xF2) {
        return false;
    }

    let Some(device_id) = read_aux_data() else {
        return false;
    };

    if device_id != 3 && device_id != 4 {
        return false;
    }

    // Включаем потоковую передачу данных.
    if !mouse_command(0xF4) {
        return false;
    }

    // Теперь разрешаем IRQ12 в контроллере.
    config |= 1 << 1;

    if !write_configuration(config) {
        return false;
    }

    PACKET_INDEX.store(0, Ordering::Relaxed);
    HEAD.store(0, Ordering::Relaxed);
    TAIL.store(0, Ordering::Relaxed);
    PKT_HEAD.store(0, Ordering::Relaxed);
    PKT_TAIL.store(0, Ordering::Relaxed);

    INITIALIZED.store(1, Ordering::Release);

    true
}

pub fn initialize() -> bool {
    // Сохраняем исходное состояние флага IF.
    let flags: u64;

    unsafe {
        asm!(
            "pushfq",
            "pop {}",
            out(reg) flags,
            options(nomem)
        );

        // Не допускаем обработку IRQ во время обмена с i8042.
        asm!("cli", options(nomem, nostack));
    }

    let result = initialize_inner();

    // Восстанавливаем исходное состояние IF.
    if flags & (1 << 9) != 0 {
        unsafe {
            asm!("sti", options(nomem, nostack));
        }
    }

    result
}

fn push_wheel(delta: i8) {
    if delta == 0 {
        return;
    }

    let head = HEAD.load(Ordering::Relaxed);
    let next = (head + 1) % QUEUE_SIZE as u8;

    if next == TAIL.load(Ordering::Acquire) {
        return;
    }

    QUEUE[head as usize].store(delta, Ordering::Relaxed);
    HEAD.store(next, Ordering::Release);
}

fn push_packet(pkt: MousePacket) {
    let head = PKT_HEAD.load(Ordering::Relaxed);
    let next = (head + 1) % PACKET_QUEUE_SIZE;
    if next == PKT_TAIL.load(Ordering::Acquire) {
        return;
    }
    PACKET_QUEUE[head].store(pkt.pack(), Ordering::Relaxed);
    PKT_HEAD.store(next, Ordering::Release);
}

#[inline]
fn process_aux_byte(byte: u8) {
    if INITIALIZED.load(Ordering::Acquire) == 0 {
        return;
    }

    let index = PACKET_INDEX.load(Ordering::Relaxed);

    // Первый байт пакета должен иметь synchronization bit.
    if index == 0 && byte & 0x08 == 0 {
        return;
    }

    PACKET[index as usize].store(byte, Ordering::Relaxed);

    if index < 3 {
        PACKET_INDEX.store(index + 1, Ordering::Relaxed);
        return;
    }

    PACKET_INDEX.store(0, Ordering::Relaxed);

    let flags = PACKET[0].load(Ordering::Relaxed);
    let raw_x = PACKET[1].load(Ordering::Relaxed);
    let raw_y = PACKET[2].load(Ordering::Relaxed);
    let wheel_byte = PACKET[3].load(Ordering::Relaxed);

    let delta_wheel = ((wheel_byte as i8) << 4) >> 4;
    push_wheel(delta_wheel);

    // Discard corrupted overflow packets, keep buttons valid.
    let (dx, dy) = if (flags & 0xC0) != 0 {
        (0i16, 0i16)
    } else {
        let mut x = raw_x as i16;
        if (flags & 0x10) != 0 {
            x |= !0xFF;
        }
        let mut y = raw_y as i16;
        if (flags & 0x20) != 0 {
            y |= !0xFF;
        }
        // PS/2 Y+ goes upward; screen Y+ goes downward.
        (x, -y)
    };

    push_packet(MousePacket {
        dx,
        dy,
        wheel: delta_wheel,
        buttons: flags & 0x07,
    });
}

pub fn handle_interrupt() {
    let status = unsafe { inb(STATUS_PORT) };
    if (status & STATUS_OUTPUT_FULL) == 0 {
        return;
    }
    if (status & STATUS_AUX_DATA) == 0 {
        return;
    }
    let byte = unsafe { inb(DATA_PORT) };
    process_aux_byte(byte);
}

/// Non-blocking recovery helper: if an AUX byte is sitting unread in i8042 `0x60`
/// (which also blocks subsequent edge-triggered IRQ12 edges), drain it into the
/// mouse packet queue safely with interrupts disabled.
pub fn poll_aux_if_pending() -> usize {
    if INITIALIZED.load(Ordering::Acquire) == 0 {
        return 0;
    }
    let first_status = unsafe { inb(STATUS_PORT) };
    if (first_status & (STATUS_OUTPUT_FULL | STATUS_AUX_DATA))
        != (STATUS_OUTPUT_FULL | STATUS_AUX_DATA)
    {
        return 0;
    }

    let mut count = 0usize;
    x86_64::instructions::interrupts::without_interrupts(|| {
        for _ in 0..16 {
            let status = unsafe { inb(STATUS_PORT) };
            if (status & (STATUS_OUTPUT_FULL | STATUS_AUX_DATA))
                != (STATUS_OUTPUT_FULL | STATUS_AUX_DATA)
            {
                break;
            }
            let byte = unsafe { inb(DATA_PORT) };
            process_aux_byte(byte);
            count += 1;
        }
    });
    count
}

pub fn read_wheel() -> Option<i8> {
    let tail = TAIL.load(Ordering::Relaxed);

    if tail == HEAD.load(Ordering::Acquire) {
        return None;
    }

    let delta = QUEUE[tail as usize].load(Ordering::Relaxed);

    TAIL.store((tail + 1) % QUEUE_SIZE as u8, Ordering::Release);

    Some(delta)
}

pub fn read_packet() -> Option<MousePacket> {
    let tail = PKT_TAIL.load(Ordering::Relaxed);
    if tail == PKT_HEAD.load(Ordering::Acquire) {
        return None;
    }
    let raw = PACKET_QUEUE[tail].load(Ordering::Relaxed);
    PKT_TAIL.store((tail + 1) % PACKET_QUEUE_SIZE, Ordering::Release);
    if (raw & (1u64 << 63)) == 0 {
        return None;
    }
    Some(MousePacket::unpack(raw))
}

pub fn drain() {
    x86_64::instructions::interrupts::without_interrupts(|| {
        PACKET_INDEX.store(0, Ordering::Relaxed);
        for _ in 0..32 {
            let status = unsafe { inb(STATUS_PORT) };
            if (status & (STATUS_OUTPUT_FULL | STATUS_AUX_DATA))
                != (STATUS_OUTPUT_FULL | STATUS_AUX_DATA)
            {
                break;
            }
            let _ = unsafe { inb(DATA_PORT) };
        }
    });
    while read_wheel().is_some() {}
    while read_packet().is_some() {}
}

pub fn is_initialized() -> bool {
    INITIALIZED.load(Ordering::Acquire) != 0
}
