
use core::arch::asm;

const PIC1_COMMAND: u16 = 0x20;
const PIC1_DATA: u16 = 0x21;
const PIC2_COMMAND: u16 = 0xA0;
const PIC2_DATA: u16 = 0xA1;

const ICW1_INITIALIZE: u8 = 0x10;
const ICW1_ICW4: u8 = 0x01;
const ICW4_8086: u8 = 0x01;

const PIC1_OFFSET: u8 = 0x20;
const PIC2_OFFSET: u8 = 0x28;

unsafe fn outb(port: u16, value: u8) {
    asm!(
        "out dx, al",
        in("dx") port,
        in("al") value,
        options(nomem, nostack, preserves_flags)
    );
}

unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    asm!(
        "in al, dx",
        out("al") value,
        in("dx") port,
        options(nomem, nostack, preserves_flags)
    );
    value
}

unsafe fn io_wait() {
    outb(0x80, 0);
}

pub fn initialize() {
    unsafe {
        outb(PIC1_COMMAND, ICW1_INITIALIZE | ICW1_ICW4);
        io_wait();

        outb(PIC2_COMMAND, ICW1_INITIALIZE | ICW1_ICW4);
        io_wait();

        outb(PIC1_DATA, PIC1_OFFSET);
        io_wait();

        outb(PIC2_DATA, PIC2_OFFSET);
        io_wait();

        outb(PIC1_DATA, 4);
        io_wait();

        outb(PIC2_DATA, 2);
        io_wait();

        outb(PIC1_DATA, ICW4_8086);
        io_wait();

        outb(PIC2_DATA, ICW4_8086);
        io_wait();

        outb(PIC1_DATA, 0xFF);
        outb(PIC2_DATA, 0xFF);
    }
}

pub fn enable_timer_and_keyboard() {
    unsafe {
        outb(PIC1_DATA, 0xFC);
        outb(PIC2_DATA, 0xFF);
    }
}

pub fn enable_mouse() {
    unsafe {
        // IRQ2: cascade from slave PIC.
        // IRQ12: PS/2 mouse.
        outb(PIC1_DATA, 0xF8);
        outb(PIC2_DATA, 0xEF);
    }
}

pub fn handle_irq7_or_spurious() {
    unsafe {
        // OCW3: read master PIC ISR (0x0B)
        outb(PIC1_COMMAND, 0x0B);
        let isr = inb(PIC1_COMMAND);
        if (isr & 0x80) != 0 {
            outb(PIC1_COMMAND, 0x20);
        }
    }
}

pub fn handle_irq15_or_spurious() {
    unsafe {
        // OCW3: read slave PIC ISR (0x0B)
        outb(PIC2_COMMAND, 0x0B);
        let isr = inb(PIC2_COMMAND);
        if (isr & 0x80) != 0 {
            outb(PIC2_COMMAND, 0x20);
        }
        // Always send EOI to master PIC for IRQ2 cascade
        outb(PIC1_COMMAND, 0x20);
    }
}

pub fn send_eoi(irq: u8) {
    unsafe {
        if irq >= 8 {
            outb(PIC2_COMMAND, 0x20);
        }

        outb(PIC1_COMMAND, 0x20);
    }
}
