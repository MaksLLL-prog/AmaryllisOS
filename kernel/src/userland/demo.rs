use alloc::vec::Vec;

use super::elf::{
    Elf64Header,
    Elf64ProgramHeader,
    ELFCLASS64,
    ELFDATA2LSB,
    ELF_MAGIC,
    EM_X86_64,
    ET_EXEC,
    PF_R,
    PF_X,
    PT_LOAD,
};

const ELF_BASE_VADDR: u64 = 0x0000_0000_0040_0000;
const CODE_FILE_OFFSET: usize = 0x80;

struct MiniElfBuilder {
    code: Vec<u8>,
    data: Vec<u8>,
    lea_patches: Vec<(usize, usize)>,      // (disp32_offset_in_code, offset_in_data)
    code_lea_patches: Vec<(usize, usize)>, // (disp32_offset_in_code, target_offset_in_code)
    os_abi: u8,
}

impl MiniElfBuilder {
    fn new() -> Self {
        Self {
            code: Vec::new(),
            data: Vec::new(),
            lea_patches: Vec::new(),
            code_lea_patches: Vec::new(),
            os_abi: 0,
        }
    }

    fn new_with_abi(os_abi: u8) -> Self {
        Self {
            code: Vec::new(),
            data: Vec::new(),
            lea_patches: Vec::new(),
            code_lea_patches: Vec::new(),
            os_abi,
        }
    }

    fn emit(&mut self, bytes: &[u8]) {
        self.code.extend_from_slice(bytes);
    }

    fn add_string(&mut self, text: &[u8]) -> (usize, u32) {
        let off = self.data.len();
        self.data.extend_from_slice(text);
        (off, text.len() as u32)
    }

    fn emit_mov_eax(&mut self, imm: u32) {
        self.emit(&[0xB8]);
        self.emit(&imm.to_le_bytes());
    }

    fn emit_mov_edi(&mut self, imm: u32) {
        self.emit(&[0xBF]);
        self.emit(&imm.to_le_bytes());
    }

    fn emit_mov_edx(&mut self, imm: u32) {
        self.emit(&[0xBA]);
        self.emit(&imm.to_le_bytes());
    }

    fn emit_lea_rsi_data(&mut self, data_off: usize) {
        // lea rsi, [rip + disp32] => 48 8D 35 disp32
        self.emit(&[0x48, 0x8D, 0x35]);
        let disp_off = self.code.len();
        self.emit(&[0, 0, 0, 0]);
        self.lea_patches.push((disp_off, data_off));
    }

    fn emit_syscall(&mut self) {
        self.emit(&[0x0F, 0x05]);
    }

    fn emit_write_literal(&mut self, msg: &[u8]) {
        let (off, len) = self.add_string(msg);
        self.emit_mov_eax(1); // SYS_WRITE
        self.emit_mov_edi(1); // stdout
        self.emit_lea_rsi_data(off);
        self.emit_mov_edx(len);
        self.emit_syscall();
    }

    fn emit_print_rax_u64_ln(&mut self) {
        // Allocates 32 bytes on user stack, converts RAX to ASCII decimal + '\n', writes to fd 1.
        self.emit(&[0x48, 0x83, 0xEC, 32]); // sub rsp, 32
        self.emit(&[0x48, 0x8D, 0x74, 0x24, 32]); // lea rsi, [rsp + 32]
        self.emit(&[0x48, 0xFF, 0xCE]); // dec rsi
        self.emit(&[0xC6, 0x06, 0x0A]); // mov byte ptr [rsi], '\n'
        self.emit(&[0x41, 0xB8, 0x01, 0x00, 0x00, 0x00]); // mov r8d, 1
        self.emit(&[0xB9, 0x0A, 0x00, 0x00, 0x00]); // mov ecx, 10

        let loop_start = self.code.len();
        self.emit(&[0x31, 0xD2]); // xor edx, edx
        self.emit(&[0x48, 0xF7, 0xF1]); // div rcx
        self.emit(&[0x80, 0xC2, 0x30]); // add dl, '0'
        self.emit(&[0x48, 0xFF, 0xCE]); // dec rsi
        self.emit(&[0x88, 0x16]); // mov [rsi], dl
        self.emit(&[0x49, 0xFF, 0xC0]); // inc r8
        self.emit(&[0x48, 0x85, 0xC0]); // test rax, rax
        let next_ip = self.code.len() + 2;
        let rel8 = (loop_start as isize - next_ip as isize) as i8 as u8;
        self.emit(&[0x75, rel8]); // jnz loop_start

        self.emit(&[0x4C, 0x89, 0xC2]); // mov rdx, r8
        self.emit_mov_eax(1); // SYS_WRITE
        self.emit_mov_edi(1); // stdout
        self.emit_syscall();
        self.emit(&[0x48, 0x83, 0xC4, 32]); // add rsp, 32
    }

    fn emit_exit(&mut self, code: u32) {
        self.emit_mov_eax(60); // SYS_EXIT
        self.emit_mov_edi(code);
        self.emit_syscall();
    }

    fn build(mut self) -> Vec<u8> {
        let data_file_offset = CODE_FILE_OFFSET + self.code.len();
        let total_size = data_file_offset + self.data.len();

        for (disp_off, data_off) in self.lea_patches {
            let next_rip = (CODE_FILE_OFFSET + disp_off + 4) as i64;
            let target = (data_file_offset + data_off) as i64;
            let disp = (target - next_rip) as i32;
            self.code[disp_off..disp_off + 4].copy_from_slice(&disp.to_le_bytes());
        }

        for (disp_off, code_off) in self.code_lea_patches {
            let next_rip = (CODE_FILE_OFFSET + disp_off + 4) as i64;
            let target = (CODE_FILE_OFFSET + code_off) as i64;
            let disp = (target - next_rip) as i32;
            self.code[disp_off..disp_off + 4].copy_from_slice(&disp.to_le_bytes());
        }

        let mut e_ident = [0u8; 16];
        e_ident[0..4].copy_from_slice(&ELF_MAGIC);
        e_ident[4] = ELFCLASS64;
        e_ident[5] = ELFDATA2LSB;
        e_ident[6] = 1; // EV_CURRENT
        e_ident[7] = self.os_abi;

        let ehdr = Elf64Header {
            e_ident,
            e_type: ET_EXEC,
            e_machine: EM_X86_64,
            e_version: 1,
            e_entry: ELF_BASE_VADDR + CODE_FILE_OFFSET as u64,
            e_phoff: core::mem::size_of::<Elf64Header>() as u64,
            e_shoff: 0,
            e_flags: 0,
            e_ehsize: core::mem::size_of::<Elf64Header>() as u16,
            e_phentsize: core::mem::size_of::<Elf64ProgramHeader>() as u16,
            e_phnum: 1,
            e_shentsize: 0,
            e_shnum: 0,
            e_shstrndx: 0,
        };

        let phdr = Elf64ProgramHeader {
            p_type: PT_LOAD,
            p_flags: PF_R | super::elf::PF_W | PF_X,
            p_offset: 0,
            p_vaddr: ELF_BASE_VADDR,
            p_paddr: ELF_BASE_VADDR,
            p_filesz: total_size as u64,
            p_memsz: total_size as u64,
            p_align: 0x1000,
        };

        let mut image = Vec::with_capacity(total_size);
        image.resize(CODE_FILE_OFFSET, 0);

        unsafe {
            let ehdr_bytes = core::slice::from_raw_parts(
                (&ehdr as *const Elf64Header) as *const u8,
                core::mem::size_of::<Elf64Header>(),
            );
            image[0..ehdr_bytes.len()].copy_from_slice(ehdr_bytes);

            let phdr_bytes = core::slice::from_raw_parts(
                (&phdr as *const Elf64ProgramHeader) as *const u8,
                core::mem::size_of::<Elf64ProgramHeader>(),
            );
            let ph_off = ehdr_bytes.len();
            image[ph_off..ph_off + phdr_bytes.len()].copy_from_slice(phdr_bytes);
        }

        image.extend_from_slice(&self.code);
        image.extend_from_slice(&self.data);
        image
    }
}

/// Standalone Ring 3 demo program: exercises SYS_WRITE, SYS_GETPID, SYS_UPTIME_MS, SYS_SLEEP_MS, SYS_EXIT.
pub fn build_hello_elf() -> Vec<u8> {
    let mut b = MiniElfBuilder::new();

    b.emit_write_literal(b"[Ring 3] Hello from AmaryllisOS Userland!\n");
    b.emit_write_literal(b"[Ring 3] Process PID: ");

    b.emit_mov_eax(39); // SYS_GETPID
    b.emit_syscall();
    b.emit_print_rax_u64_ln();

    b.emit_write_literal(b"[Ring 3] Kernel uptime (ms): ");
    b.emit_mov_eax(96); // SYS_UPTIME_MS
    b.emit_syscall();
    b.emit_print_rax_u64_ln();

    b.emit_write_literal(b"[Ring 3] Sleeping 200 ms via SYS_SLEEP_MS...\n");
    b.emit_mov_eax(35); // SYS_SLEEP_MS
    b.emit_mov_edi(200);
    b.emit_syscall();

    b.emit_write_literal(b"[Ring 3] Exiting cleanly with status 0.\n");
    b.emit_exit(0);

    b.build()
}

/// Interactive Ring 3 echo program: exercises SYS_READ (stdin) + SYS_WRITE + SYS_EXIT.
pub fn build_echo_elf() -> Vec<u8> {
    let mut b = MiniElfBuilder::new();

    b.emit_write_literal(b"[Ring 3 echo] Enter a line: ");

    // Reserve 128-byte user stack buffer and call SYS_READ(0, rsp, 127)
    b.emit(&[0x48, 0x81, 0xEC, 0x80, 0x00, 0x00, 0x00]); // sub rsp, 128
    b.emit_mov_eax(0); // SYS_READ
    b.emit_mov_edi(0); // stdin
    b.emit(&[0x48, 0x89, 0xE6]); // mov rsi, rsp
    b.emit_mov_edx(127);
    b.emit_syscall();

    // Preserve byte count read in R12
    b.emit(&[0x49, 0x89, 0xC4]); // mov r12, rax

    b.emit_write_literal(b"[Ring 3 echo] Read from user buffer: ");

    b.emit_mov_eax(1); // SYS_WRITE
    b.emit_mov_edi(1); // stdout
    b.emit(&[0x48, 0x89, 0xE6]); // mov rsi, rsp
    b.emit(&[0x4C, 0x89, 0xE2]); // mov rdx, r12
    b.emit_syscall();

    b.emit(&[0x48, 0x81, 0xC4, 0x80, 0x00, 0x00, 0x00]); // add rsp, 128
    b.emit_exit(0);

    b.build()
}

/// Memory-protection test ELF: writes to NULL (0x0) from Ring 3 to prove hardware #PF isolation.
pub fn build_fault_elf() -> Vec<u8> {
    let mut b = MiniElfBuilder::new();

    b.emit_write_literal(b"[Ring 3 fault] Triggering intentional Ring 3 #PF at 0x0...\n");
    b.emit(&[0x31, 0xC0]); // xor eax, eax
    b.emit(&[0x48, 0xC7, 0x00, 0x34, 0x12, 0x00, 0x00]); // mov qword ptr [rax], 0x1234
    b.emit_exit(1);

    b.build()
}

/// Interactive Ring 3 GUI ELF: creates a desktop window (320x180), animates plasma waves + clickable glow,
/// handles CloseRequested (or 'q') cleanly via SYS_CREATE_WINDOW(300) / SYS_WINDOW_BLIT(301) / SYS_POLL_GUI_EVENT(302).
pub fn build_gui_demo_elf() -> Vec<u8> {
    const W: usize = 320;
    const H: usize = 180;
    let mut b = MiniElfBuilder::new();

    let (title_off, title_len) = b.add_string(b"Amaryllis Ring 3 Demo (click / q)");
    let fb_off = b.data.len();
    b.data.resize(fb_off + W * H * 4, 0);

    // Reserve stack space: 16 bytes for UserGuiEvent
    b.emit(&[0x48, 0x83, 0xEC, 32]); // sub rsp, 32

    // SYS_CREATE_WINDOW(title_ptr, title_len, (W << 16) | H) -> win_id in r12
    b.emit_mov_eax(300);
    // lea rdi, [rip + disp32] => 48 8D 3D disp32
    b.emit(&[0x48, 0x8D, 0x3D]);
    let t_disp = b.code.len();
    b.emit(&[0, 0, 0, 0]);
    b.lea_patches.push((t_disp, title_off));
    // mov esi, title_len
    b.emit(&[0xBE]);
    b.emit(&title_len.to_le_bytes());
    b.emit_mov_edx(((W as u32) << 16) | (H as u32));
    b.emit_syscall();
    // If rax < 0, exit(1)
    b.emit(&[0x48, 0x85, 0xC0]); // test rax, rax
    let jns_ok = b.code.len();
    b.emit(&[0x79, 0x0C]); // jns +12 (skip exit)
    b.emit_mov_eax(60);
    b.emit_mov_edi(1);
    b.emit_syscall();
    let ok_pos = b.code.len();
    b.code[jns_ok + 1] = (ok_pos - (jns_ok + 2)) as u8;

    // r12 = win_id, r13 = frame counter, r14 = beacon_x (160), r15 = beacon_y (90)
    b.emit(&[0x49, 0x89, 0xC4]); // mov r12, rax
    b.emit(&[0x45, 0x31, 0xED]); // xor r13d, r13d
    b.emit(&[0x41, 0xBE, 160, 0, 0, 0]); // mov r14d, 160
    b.emit(&[0x41, 0xBF, 90, 0, 0, 0]); // mov r15d, 90

    // Frame loop start
    let frame_loop = b.code.len();

    // rbx = framebuffer pointer
    b.emit(&[0x48, 0x8D, 0x1D]); // lea rbx, [rip + disp32]
    let fb_disp = b.code.len();
    b.emit(&[0, 0, 0, 0]);
    b.lea_patches.push((fb_disp, fb_off));

    // Loop y = 0..H (ecx = y)
    b.emit(&[0x31, 0xC9]); // xor ecx, ecx
    let y_loop = b.code.len();
    // Loop x = 0..W (r8d = x)
    b.emit(&[0x45, 0x31, 0xC0]); // xor r8d, r8d
    let x_loop = b.code.len();

    // Compute pixel color in eax without fragile SIB lea:
    // edx = (r8d + r13d) ^ (ecx + r13d)
    b.emit(&[0x44, 0x89, 0xC0]); // mov eax, r8d
    b.emit(&[0x44, 0x01, 0xE8]); // add eax, r13d
    b.emit(&[0x89, 0xCA]); // mov edx, ecx
    b.emit(&[0x44, 0x01, 0xEA]); // add edx, r13d
    b.emit(&[0x31, 0xD0]); // xor eax, edx
    b.emit(&[0x83, 0xE0, 0x3F]); // and eax, 0x3F
    b.emit(&[0x83, 0xC0, 0x12]); // add eax, 0x12 (R)
    b.emit(&[0xC1, 0xE0, 16]); // shl eax, 16

    // G channel in edx
    b.emit(&[0x44, 0x89, 0xC2]); // mov edx, r8d
    b.emit(&[0x44, 0x01, 0xEA]); // add edx, r13d
    b.emit(&[0x83, 0xE2, 0x7F]); // and edx, 0x7F
    b.emit(&[0x83, 0xC2, 0x38]); // add edx, 0x38
    b.emit(&[0xC1, 0xE2, 8]); // shl edx, 8
    b.emit(&[0x09, 0xD0]); // or eax, edx

    // B channel in edx
    b.emit(&[0x89, 0xCA]); // mov edx, ecx
    b.emit(&[0x44, 0x01, 0xEA]); // add edx, r13d
    b.emit(&[0x83, 0xE2, 0x7F]); // and edx, 0x7F
    b.emit(&[0x83, 0xC2, 0x55]); // add edx, 0x55
    b.emit(&[0x09, 0xD0]); // or eax, edx

    // Check Manhattan distance to beacon (r14d, r15d)
    b.emit(&[0x44, 0x89, 0xC2]); // mov edx, r8d
    b.emit(&[0x44, 0x29, 0xF2]); // sub edx, r14d
    b.emit(&[0x85, 0xD2]); // test edx, edx
    b.emit(&[0x79, 0x02]); // jns +2
    b.emit(&[0xF7, 0xDA]); // neg edx

    b.emit(&[0x89, 0xCE]); // mov esi, ecx
    b.emit(&[0x44, 0x29, 0xFE]); // sub esi, r15d
    b.emit(&[0x85, 0xF6]); // test esi, esi
    b.emit(&[0x79, 0x02]); // jns +2
    b.emit(&[0xF7, 0xDE]); // neg esi

    b.emit(&[0x01, 0xF2]); // add edx, esi
    b.emit(&[0x83, 0xFA, 24]); // cmp edx, 24
    b.emit(&[0x7D, 0x05]); // jge +5
    b.emit(&[0xB8, 0x96, 0xE8, 0x28, 0x00]); // mov eax, 0x0028E896 (Amaryllis mint glow)

    // Write pixel: mov [rbx], eax; add rbx, 4
    b.emit(&[0x89, 0x03]);
    b.emit(&[0x48, 0x83, 0xC3, 4]);

    // inc r8d; cmp r8d, W; jl x_loop
    b.emit(&[0x41, 0xFF, 0xC0]);
    b.emit(&[0x41, 0x81, 0xF8]);
    b.emit(&(W as u32).to_le_bytes());
    let j_x = b.code.len() + 6;
    let rel_x = (x_loop as isize - j_x as isize) as i32;
    b.emit(&[0x0F, 0x8C]);
    b.emit(&rel_x.to_le_bytes());

    // inc ecx; cmp ecx, H; jl y_loop
    b.emit(&[0xFF, 0xC1]);
    b.emit(&[0x81, 0xF9]);
    b.emit(&(H as u32).to_le_bytes());
    let j_y = b.code.len() + 6;
    let rel_y = (y_loop as isize - j_y as isize) as i32;
    b.emit(&[0x0F, 0x8C]);
    b.emit(&rel_y.to_le_bytes());

    // SYS_WINDOW_BLIT(win_id=r12, buf_ptr, (W<<16)|H)
    b.emit_mov_eax(301);
    b.emit(&[0x4C, 0x89, 0xE7]); // mov rdi, r12
    b.emit_lea_rsi_data(fb_off);
    b.emit_mov_edx(((W as u32) << 16) | (H as u32));
    b.emit_syscall();

    // SYS_POLL_GUI_EVENT(win_id=r12, rsp, 20)
    b.emit_mov_eax(302);
    b.emit(&[0x4C, 0x89, 0xE7]); // mov rdi, r12
    b.emit(&[0x48, 0x89, 0xE6]); // mov rsi, rsp
    b.emit_mov_edx(20);
    b.emit_syscall();

    // Check event
    b.emit(&[0x8B, 0x04, 0x24]); // mov eax, [rsp] (kind)
    b.emit(&[0x83, 0xF8, 1]); // cmp eax, 1 (CloseRequested)
    let je_exit1 = b.code.len();
    b.emit(&[0x0F, 0x84, 0, 0, 0, 0]); // patched later

    b.emit(&[0x83, 0xF8, 3]); // cmp eax, 3 (KeyChar)
    let jne_key = b.code.len();
    b.emit(&[0x75, 0x00]); // dynamically patched below
    b.emit(&[0x83, 0x7C, 0x24, 4, b'q' as u8]); // cmp dword ptr [rsp+4], 'q'
    let je_exit2 = b.code.len();
    b.emit(&[0x0F, 0x84, 0, 0, 0, 0]); // patched later
    let after_key = b.code.len();
    b.code[jne_key + 1] = (after_key - (jne_key + 2)) as u8;

    // If MouseDown (kind == 2): update r14d = [rsp+4], r15d = [rsp+8], beep
    b.emit(&[0x83, 0xF8, 2]); // cmp eax, 2
    let jne_next = b.code.len();
    b.emit(&[0x75, 0x00]); // dynamically patched below
    b.emit(&[0x44, 0x8B, 0x74, 0x24, 4]); // mov r14d, [rsp+4]
    b.emit(&[0x44, 0x8B, 0x7C, 0x24, 8]); // mov r15d, [rsp+8]
    b.emit_mov_eax(200); // SYS_BEEP
    b.emit_mov_edi(680);
    b.emit(&[0xBE, 35, 0, 0, 0]); // mov esi, 35
    b.emit_syscall();
    let after_mouse = b.code.len();
    b.code[jne_next + 1] = (after_mouse - (jne_next + 2)) as u8;

    // inc r13d; jmp frame_loop
    b.emit(&[0x41, 0xFF, 0xC5]); // inc r13d
    let jmp_end = b.code.len() + 5;
    let rel_loop = (frame_loop as isize - jmp_end as isize) as i32;
    b.emit(&[0xE9]);
    b.emit(&rel_loop.to_le_bytes());

    // Exit label
    let exit_label = b.code.len();
    let disp1 = (exit_label as isize - (je_exit1 + 6) as isize) as i32;
    b.code[je_exit1 + 2..je_exit1 + 6].copy_from_slice(&disp1.to_le_bytes());
    let disp2 = (exit_label as isize - (je_exit2 + 6) as isize) as i32;
    b.code[je_exit2 + 2..je_exit2 + 6].copy_from_slice(&disp2.to_le_bytes());

    b.emit(&[0x48, 0x83, 0xC4, 32]); // add rsp, 32
    b.emit_exit(0);

    b.build()
}

/// Preemptive multitasking counter ELF: loops 8 iterations printing its PID and uptime every 350 ms.
/// Launch two copies with 'execbg /counter.elf' or Files to observe concurrent Ring 3 scheduling!
pub fn build_counter_elf() -> Vec<u8> {
    let mut b = MiniElfBuilder::new();

    // Reserve r12 = loop counter (1..=8), r13 = my pid
    b.emit_mov_eax(39); // SYS_GETPID
    b.emit_syscall();
    b.emit(&[0x49, 0x89, 0xC5]); // mov r13, rax

    b.emit_write_literal(b"[counter] Multitasking Ring 3 worker started, PID=");
    b.emit(&[0x4C, 0x89, 0xE8]); // mov rax, r13
    b.emit_print_rax_u64_ln();

    b.emit(&[0x41, 0xBC, 0x01, 0x00, 0x00, 0x00]); // mov r12d, 1

    let loop_top = b.code.len();
    b.emit_write_literal(b"  PID=");
    b.emit(&[0x4C, 0x89, 0xE8]); // mov rax, r13
    b.emit_print_rax_u64_ln();

    b.emit_write_literal(b"  step=");
    b.emit(&[0x4C, 0x89, 0xE0]); // mov rax, r12
    b.emit_print_rax_u64_ln();

    b.emit_write_literal(b"  uptime_ms=");
    b.emit_mov_eax(96); // SYS_UPTIME_MS
    b.emit_syscall();
    b.emit_print_rax_u64_ln();

    // SYS_SLEEP_MS(350)
    b.emit_mov_eax(35);
    b.emit_mov_edi(350);
    b.emit_syscall();

    // inc r12d; cmp r12d, 9; jl loop_top
    b.emit(&[0x41, 0xFF, 0xC4]);
    b.emit(&[0x41, 0x83, 0xFC, 9]);
    let jmp_end = b.code.len() + 6;
    let rel = (loop_top as isize - jmp_end as isize) as i32;
    b.emit(&[0x0F, 0x8C]);
    b.emit(&rel.to_le_bytes());

    b.emit_write_literal(b"[counter] Worker finished cleanly.\n");
    b.emit_exit(0);
    b.build()
}

/// Ring 3 Network syscall demo ELF: exercises SYS_NET_PING(210) on QEMU SLIRP gateway 10.0.2.2
/// and SYS_DNS_RESOLVE(211) on "example.com".
pub fn build_net_demo_elf() -> Vec<u8> {
    let mut b = MiniElfBuilder::new();
    let (host_off, host_len) = b.add_string(b"example.com");

    b.emit_write_literal(b"[net_demo] Ring 3 Network Syscall Demo\n");
    b.emit_write_literal(b"[net_demo] Pinging 10.0.2.2 via SYS_NET_PING (210)...\n");

    // SYS_NET_PING(0x0A000202, seq=1, timeout_ms=1200)
    b.emit_mov_eax(210);
    b.emit_mov_edi(0x0A00_0202);
    b.emit(&[0xBE, 1, 0, 0, 0]); // mov esi, 1
    b.emit_mov_edx(1200);
    b.emit_syscall();

    b.emit(&[0x48, 0x85, 0xC0]); // test rax, rax
    let jns_ok = b.code.len();
    b.emit(&[0x79, 0x00]);
    b.emit_write_literal(b"[net_demo] Ping timed out (run 'dhcp' or check e1000)\n");
    let jmp_dns = b.code.len();
    b.emit(&[0xEB, 0x00]);

    let ping_ok = b.code.len();
    b.code[jns_ok + 1] = (ping_ok - (jns_ok + 2)) as u8;
    b.emit(&[0x49, 0x89, 0xC4]); // mov r12, rax
    b.emit_write_literal(b"[net_demo] Ping OK, RTT(ms)=");
    b.emit(&[0x4C, 0x89, 0xE0]); // mov rax, r12
    b.emit_print_rax_u64_ln();

    let dns_label = b.code.len();
    b.code[jmp_dns + 1] = (dns_label - (jmp_dns + 2)) as u8;

    b.emit_write_literal(b"[net_demo] Resolving example.com via SYS_DNS_RESOLVE (211)...\n");
    b.emit(&[0x48, 0x83, 0xEC, 16]); // sub rsp, 16 (4-byte IP out buffer)
    b.emit_mov_eax(211);
    b.emit(&[0x48, 0x8D, 0x3D]); // lea rdi, [rip + disp32]
    let h_disp = b.code.len();
    b.emit(&[0, 0, 0, 0]);
    b.lea_patches.push((h_disp, host_off));
    b.emit(&[0xBE]);
    b.emit(&host_len.to_le_bytes());
    b.emit(&[0x48, 0x89, 0xE2]); // mov rdx, rsp
    b.emit_syscall();

    b.emit(&[0x48, 0x85, 0xC0]); // test rax, rax
    let jz_dns_ok = b.code.len();
    b.emit(&[0x74, 0x00]);
    b.emit_write_literal(b"[net_demo] DNS lookup failed or timed out.\n");
    let jmp_done = b.code.len();
    b.emit(&[0xEB, 0x00]);

    let dns_ok = b.code.len();
    b.code[jz_dns_ok + 1] = (dns_ok - (jz_dns_ok + 2)) as u8;
    b.emit_write_literal(b"[net_demo] DNS resolved first octet=");
    b.emit(&[0x0F, 0xB6, 0x04, 0x24]); // movzx eax, byte ptr [rsp]
    b.emit_print_rax_u64_ln();

    let done_label = b.code.len();
    b.code[jmp_done + 1] = (done_label - (jmp_done + 2)) as u8;
    b.emit(&[0x48, 0x83, 0xC4, 16]);
    b.emit_exit(0);
    b.build()
}

/// Demand-paged lazy allocation & hardened copy_*_user demo ELF (`/lazy_demo.elf`):
/// 1) Reserves 128 KiB via SYS_MMAP(9) (0 physical pages committed initially)
/// 2) Touches bytes across page 0 & page 31, triggering hardware #PF demand-paging
/// 3) Expands user stack by -68 KiB cleanly via lazy stack growth #PF
/// 4) Calls SYS_STAT(4) pointing into an untouched lazy mmap page (copy_to_user commits on demand!)
pub fn build_lazy_demo_elf() -> Vec<u8> {
    let mut b = MiniElfBuilder::new();
    let (stat_path_off, stat_path_len) = b.add_string(b"/disk");

    b.emit_write_literal(b"[lazy_demo] Demand-Paged #PF + Hardened copy_*_user Test\n");
    b.emit_write_literal(b"[lazy_demo] Calling SYS_MMAP(131072, RW)...\n");

    // SYS_MMAP(addr=0, len=131072, prot=3)
    b.emit_mov_eax(9);
    b.emit_mov_edi(0);
    b.emit(&[0xBE, 0x00, 0x00, 0x02, 0x00]); // mov esi, 131072
    b.emit_mov_edx(3);
    b.emit_syscall();

    // Save mmap base in r12
    b.emit(&[0x49, 0x89, 0xC4]); // mov r12, rax
    b.emit_write_literal(b"[lazy_demo] mmap base (0 pages mapped yet)=");
    b.emit(&[0x4C, 0x89, 0xE0]); // mov rax, r12
    b.emit_print_rax_u64_ln();

    // 1) Touch page 0 and page 31 (triggers 2 demand-paged #PFs directly from Ring 3!)
    b.emit(&[0x41, 0xC7, 0x04, 0x24, 0x42, 0x00, 0x00, 0x00]); // mov dword ptr [r12], 0x42
    b.emit(&[0x49, 0xC7, 0x84, 0x24, 0x00, 0xF0, 0x01, 0x00, 0x99, 0x00, 0x00, 0x00]); // mov qword ptr [r12 + 0x1F000], 0x99

    // 2) Lazy stack expansion (-68 KiB crosses initial 64 KiB mapped boundary!)
    b.emit(&[0x48, 0x81, 0xEC, 0x00, 0x10, 0x01, 0x00]); // sub rsp, 0x11000
    b.emit(&[0x48, 0xC7, 0x04, 0x24, 0x55, 0x00, 0x00, 0x00]); // mov qword ptr [rsp], 0x55
    b.emit(&[0x48, 0x81, 0xC4, 0x00, 0x10, 0x01, 0x00]); // add rsp, 0x11000

    b.emit_write_literal(b"[lazy_demo] Touched mmap p0+p31 & -68KiB stack cleanly!\n");

    // 3) Hardened copy_to_user onto untouched lazy mmap page 16 (r12 + 0x10000)
    b.emit_mov_eax(4); // SYS_STAT("/disk", len, r12 + 0x10000)
    b.emit(&[0x48, 0x8D, 0x3D]); // lea rdi, [rip + disp32]
    let p_disp = b.code.len();
    b.emit(&[0, 0, 0, 0]);
    b.lea_patches.push((p_disp, stat_path_off));
    b.emit(&[0xBE]);
    b.emit(&stat_path_len.to_le_bytes());
    b.emit(&[0x49, 0x8D, 0x94, 0x24, 0x00, 0x00, 0x01, 0x00]); // lea rdx, [r12 + 0x10000]
    b.emit_syscall();

    b.emit(&[0x48, 0x85, 0xC0]); // test rax, rax
    let jnz_stat_fail = b.code.len();
    b.emit(&[0x75, 0x00]);
    b.emit_write_literal(b"[lazy_demo] SYS_STAT onto untouched lazy page OK, bytes=");
    b.emit(&[0x49, 0x8B, 0x84, 0x24, 0x00, 0x00, 0x01, 0x00]); // mov rax, [r12 + 0x10000]
    b.emit_print_rax_u64_ln();
    let jmp_unmap = b.code.len();
    b.emit(&[0xEB, 0x00]);

    let fail_lbl = b.code.len();
    b.code[jnz_stat_fail + 1] = (fail_lbl - (jnz_stat_fail + 2)) as u8;
    b.emit_write_literal(b"[lazy_demo] SYS_STAT returned error\n");

    let unmap_lbl = b.code.len();
    b.code[jmp_unmap + 1] = (unmap_lbl - (jmp_unmap + 2)) as u8;
    // SYS_MUNMAP(r12, 131072)
    b.emit_mov_eax(11);
    b.emit(&[0x4C, 0x89, 0xE7]); // mov rdi, r12
    b.emit(&[0xBE, 0x00, 0x00, 0x02, 0x00]); // mov esi, 131072
    b.emit_syscall();

    b.emit_write_literal(b"[lazy_demo] All demand-paged & guard checks PASSED!\n");
    b.emit_exit(0);
    b.build()
}

/// Intentional User Stack Guard Page trigger ELF (`/guard_fault.elf`):
/// Probes the unmapped 4 KiB guard page at `0x0000_007F_FFFA_F000` just below the 256 KiB stack limit
/// so the kernel intercepts `#PF` (`GUARD_PAGE_HIT`) cleanly without corrupting any state.
pub fn build_guard_fault_elf() -> Vec<u8> {
    let mut b = MiniElfBuilder::new();
    b.emit_write_literal(b"[guard_fault] Probing User Stack Guard Page (0x7FFFFAF000)...\n");
    // mov rax, 0x0000007FFFFAF000 -> 48 B8 <u64>
    b.emit(&[0x48, 0xB8]);
    b.emit(&super::elf::USER_STACK_GUARD_ADDR.to_le_bytes());
    // mov qword ptr [rax], 1 => triggers hardware #PF in StackGuard VMA
    b.emit(&[0x48, 0xC7, 0x00, 0x01, 0x00, 0x00, 0x00]);
    b.emit_exit(1);
    b.build()
}

/// CoW `fork()` + `waitpid()` verification ELF (`/fork_demo.elf`):
/// 1) Maps an RW page and writes `0x11223344`
/// 2) Forks with CoW (`SYS_FORK`)
/// 3) Child writes `0x55667788` (copy-on-write private page fault) and exits with code 42
/// 4) Parent reaps child via `SYS_WAITPID` and verifies parent's page still reads `0x11223344`
pub fn build_fork_demo_elf() -> Vec<u8> {
    let mut b = MiniElfBuilder::new();
    b.emit_write_literal(b"[fork_demo] Starting CoW fork() + waitpid() test...\n");

    // SYS_MMAP(addr=0, len=4096, prot=3)
    b.emit_mov_eax(9);
    b.emit_mov_edi(0);
    b.emit(&[0xBE, 0x00, 0x10, 0x00, 0x00]); // mov esi, 4096
    b.emit_mov_edx(3);
    b.emit_syscall();

    b.emit(&[0x48, 0x85, 0xC0]); // test rax, rax
    let jns_mmap = b.code.len();
    b.emit(&[0x0F, 0x89, 0, 0, 0, 0]); // jns mmap_ok
    b.emit_write_literal(b"[fork_demo] mmap failed!\n");
    b.emit_exit(1);

    let mmap_ok = b.code.len();
    let d = (mmap_ok as isize - (jns_mmap + 6) as isize) as i32;
    b.code[jns_mmap + 2..jns_mmap + 6].copy_from_slice(&d.to_le_bytes());

    // r12 = shared CoW test page; store 0x11223344
    b.emit(&[0x49, 0x89, 0xC4]); // mov r12, rax
    b.emit(&[0x41, 0xC7, 0x04, 0x24, 0x44, 0x33, 0x22, 0x11]); // mov dword ptr [r12], 0x11223344

    // SYS_FORK (57)
    b.emit_mov_eax(57);
    b.emit_syscall();

    b.emit(&[0x48, 0x85, 0xC0]); // test rax, rax
    let jz_child = b.code.len();
    b.emit(&[0x0F, 0x84, 0, 0, 0, 0]); // je child_path

    let jns_parent = b.code.len();
    b.emit(&[0x0F, 0x89, 0, 0, 0, 0]); // jns parent_path
    b.emit_write_literal(b"[fork_demo] SYS_FORK failed!\n");
    b.emit_exit(2);

    // Parent path
    let parent_lbl = b.code.len();
    let d_parent = (parent_lbl as isize - (jns_parent + 6) as isize) as i32;
    b.code[jns_parent + 2..jns_parent + 6].copy_from_slice(&d_parent.to_le_bytes());

    b.emit(&[0x49, 0x89, 0xC5]); // mov r13, rax (child_pid)
    b.emit(&[0x48, 0x83, 0xEC, 16]); // sub rsp, 16
    b.emit_mov_eax(61); // SYS_WAITPID
    b.emit(&[0x4C, 0x89, 0xEF]); // mov rdi, r13
    b.emit(&[0x48, 0x89, 0xE6]); // mov rsi, rsp
    b.emit(&[0x31, 0xD2]); // xor edx, edx
    b.emit_syscall();

    b.emit(&[0x44, 0x8B, 0x34, 0x24]); // mov r14d, dword ptr [rsp]
    b.emit(&[0x48, 0x83, 0xC4, 16]); // add rsp, 16

    b.emit_write_literal(b"[fork_demo parent] Reaped child, exit code=");
    b.emit(&[0x44, 0x89, 0xF0]); // mov eax, r14d
    b.emit_print_rax_u64_ln();

    // Verify parent's CoW page still contains 0x11223344
    b.emit(&[0x41, 0x8B, 0x04, 0x24]); // mov eax, dword ptr [r12]
    b.emit(&[0x3D, 0x44, 0x33, 0x22, 0x11]); // cmp eax, 0x11223344
    let je_cow_ok = b.code.len();
    b.emit(&[0x0F, 0x84, 0, 0, 0, 0]); // je cow_ok

    b.emit_write_literal(b"[fork_demo parent] CoW isolation FAILED!\n");
    b.emit_mov_eax(11); // SYS_MUNMAP
    b.emit(&[0x4C, 0x89, 0xE7]); // mov rdi, r12
    b.emit(&[0xBE, 0x00, 0x10, 0x00, 0x00]); // mov esi, 4096
    b.emit_syscall();
    b.emit_exit(3);

    let cow_ok_lbl = b.code.len();
    let d_cow = (cow_ok_lbl as isize - (je_cow_ok + 6) as isize) as i32;
    b.code[je_cow_ok + 2..je_cow_ok + 6].copy_from_slice(&d_cow.to_le_bytes());

    b.emit_write_literal(b"[fork_demo parent] CoW value preserved (0x11223344) PASS!\n");
    b.emit_mov_eax(11); // SYS_MUNMAP
    b.emit(&[0x4C, 0x89, 0xE7]); // mov rdi, r12
    b.emit(&[0xBE, 0x00, 0x10, 0x00, 0x00]); // mov esi, 4096
    b.emit_syscall();
    b.emit_exit(0);

    // Child path
    let child_lbl = b.code.len();
    let d_child = (child_lbl as isize - (jz_child + 6) as isize) as i32;
    b.code[jz_child + 2..jz_child + 6].copy_from_slice(&d_child.to_le_bytes());

    // Trigger CoW fault on [r12]
    b.emit(&[0x41, 0xC7, 0x04, 0x24, 0x88, 0x77, 0x66, 0x55]); // mov dword ptr [r12], 0x55667788
    b.emit_write_literal(b"[fork_demo child] Wrote private CoW copy (0x55667788), PID=");
    b.emit_mov_eax(39); // SYS_GETPID
    b.emit_syscall();
    b.emit_print_rax_u64_ln();
    b.emit_exit(42);

    b.build()
}

/// Mach IPC Ports + Zero-Copy OOL Page Transfer + Bootstrap Registry Demo (`/mach_ipc_demo.elf`):
/// 1) Parent allocates a Mach port and registers `"demo.mach.svc"` in Bootstrap
/// 2) Parent forks; child looks up `"demo.mach.svc"`, maps a 4 KiB page (`0xCAFEBABE`),
///    and performs an atomic `SYS_MACH_MSG` RPC (`SEND | RCV`) with complex OOL descriptor
/// 3) Parent receives the OOL page zero-copy, validates `0xCAFEBABE`, replies on `SendOnce`,
///    and reaps child via `SYS_WAITPID`.
pub fn build_mach_ipc_demo_elf() -> Vec<u8> {
    let mut b = MiniElfBuilder::new();
    let (svc_off, svc_len) = b.add_string(b"demo.mach.svc");

    b.emit_write_literal(b"[mach_demo] Allocating Mach port & registering 'demo.mach.svc'...\n");

    // r12 = SYS_MACH_PORT_ALLOCATE (330)
    b.emit_mov_eax(330);
    b.emit_syscall();
    b.emit(&[0x48, 0x85, 0xC0]); // test rax, rax
    let jg_alloc = b.code.len();
    b.emit(&[0x0F, 0x8F, 0, 0, 0, 0]);
    b.emit_write_literal(b"[mach_demo] port allocate failed\n");
    b.emit_exit(1);

    let alloc_ok = b.code.len();
    let d_alloc = (alloc_ok as isize - (jg_alloc + 6) as isize) as i32;
    b.code[jg_alloc + 2..jg_alloc + 6].copy_from_slice(&d_alloc.to_le_bytes());
    b.emit(&[0x49, 0x89, 0xC4]); // mov r12, rax

    // SYS_BOOTSTRAP_REGISTER (333)(svc_ptr, svc_len, r12)
    b.emit_mov_eax(333);
    b.emit(&[0x48, 0x8D, 0x3D]); // lea rdi, [rip + disp32]
    let p_reg = b.code.len();
    b.emit(&[0, 0, 0, 0]);
    b.lea_patches.push((p_reg, svc_off));
    b.emit(&[0xBE]);
    b.emit(&svc_len.to_le_bytes());
    b.emit(&[0x4C, 0x89, 0xE2]); // mov rdx, r12
    b.emit_syscall();

    // SYS_FORK (57)
    b.emit_mov_eax(57);
    b.emit_syscall();
    b.emit(&[0x48, 0x85, 0xC0]); // test rax, rax
    let jz_child = b.code.len();
    b.emit(&[0x0F, 0x84, 0, 0, 0, 0]); // je child_lbl

    // Parent (Server) path: r13 = child_pid
    b.emit(&[0x49, 0x89, 0xC5]); // mov r13, rax
    b.emit(&[0x48, 0x81, 0xEC, 0x80, 0x00, 0x00, 0x00]); // sub rsp, 128

    // SYS_MACH_MSG(rsp, MACH_RCV_MSG=2, send=0, rcv_cap=128, rcv_name=r12, timeout=0)
    b.emit_mov_eax(332);
    b.emit(&[0x48, 0x89, 0xE7]); // mov rdi, rsp
    b.emit(&[0xBE, 0x02, 0x00, 0x00, 0x00]); // mov esi, 2
    b.emit_mov_edx(0);
    b.emit(&[0x41, 0xBA, 0x80, 0x00, 0x00, 0x00]); // mov r10d, 128
    b.emit(&[0x4D, 0x89, 0xE0]); // mov r8, r12
    b.emit(&[0x45, 0x31, 0xC9]); // xor r9d, r9d
    b.emit_syscall();

    b.emit(&[0x48, 0x85, 0xC0]); // test rax, rax
    let jns_rcv = b.code.len();
    b.emit(&[0x0F, 0x89, 0, 0, 0, 0]);
    b.emit_write_literal(b"[mach_demo server] recv failed\n");
    b.emit(&[0x48, 0x81, 0xC4, 0x80, 0x00, 0x00, 0x00]);
    b.emit_exit(2);

    let rcv_ok = b.code.len();
    let d_rcv = (rcv_ok as isize - (jns_rcv + 6) as isize) as i32;
    b.code[jns_rcv + 2..jns_rcv + 6].copy_from_slice(&d_rcv.to_le_bytes());

    // ebx = header.local_port ([rsp+12]), rbp = ool.addr ([rsp+24])
    b.emit(&[0x8B, 0x5C, 0x24, 0x0C]); // mov ebx, dword ptr [rsp + 12]
    b.emit(&[0x48, 0x8B, 0x6C, 0x24, 0x18]); // mov rbp, qword ptr [rsp + 24]

    // Check [rbp] == 0xCAFEBABE
    b.emit(&[0x81, 0x7D, 0x00, 0xBE, 0xBA, 0xFE, 0xCA]); // cmp dword ptr [rbp], 0xCAFEBABE
    let je_ool = b.code.len();
    b.emit(&[0x0F, 0x84, 0, 0, 0, 0]);
    b.emit_write_literal(b"[mach_demo server] OOL payload mismatch!\n");
    b.emit(&[0x48, 0x81, 0xC4, 0x80, 0x00, 0x00, 0x00]);
    b.emit_exit(3);

    let ool_ok = b.code.len();
    let d_ool = (ool_ok as isize - (je_ool + 6) as isize) as i32;
    b.code[je_ool + 2..je_ool + 6].copy_from_slice(&d_ool.to_le_bytes());

    b.emit_write_literal(b"[mach_demo server] Zero-copy OOL page verified (0xCAFEBABE), replying...\n");

    // SYS_MUNMAP(rbp, 4096)
    b.emit_mov_eax(11);
    b.emit(&[0x48, 0x89, 0xEF]); // mov rdi, rbp
    b.emit(&[0xBE, 0x00, 0x10, 0x00, 0x00]); // mov esi, 4096
    b.emit_syscall();

    // Build simple reply MachMsgHeader at [rsp..rsp+24]
    b.emit(&[0xC7, 0x04, 0x24, 18, 0, 0, 0]); // bits = MOVE_SEND_ONCE (18)
    b.emit(&[0xC7, 0x44, 0x24, 4, 24, 0, 0, 0]); // size = 24
    b.emit(&[0x89, 0x5C, 0x24, 8]); // remote_port = ebx
    b.emit(&[0xC7, 0x44, 0x24, 12, 0, 0, 0, 0]); // local_port = 0
    b.emit(&[0xC7, 0x44, 0x24, 16, 0x09, 0x03, 0, 0]); // id = 777
    b.emit(&[0xC7, 0x44, 0x24, 20, 0, 0, 0, 0]); // reserved = 0

    // SYS_MACH_MSG(rsp, MACH_SEND_MSG=1, 24, 0, 0, 0)
    b.emit_mov_eax(332);
    b.emit(&[0x48, 0x89, 0xE7]); // mov rdi, rsp
    b.emit(&[0xBE, 0x01, 0x00, 0x00, 0x00]); // mov esi, 1
    b.emit_mov_edx(24);
    b.emit(&[0x45, 0x31, 0xD2]); // xor r10d, r10d
    b.emit(&[0x45, 0x31, 0xC0]); // xor r8d, r8d
    b.emit(&[0x45, 0x31, 0xC9]); // xor r9d, r9d
    b.emit_syscall();

    // SYS_WAITPID(r13, rsp, 0)
    b.emit_mov_eax(61);
    b.emit(&[0x4C, 0x89, 0xEF]); // mov rdi, r13
    b.emit(&[0x48, 0x89, 0xE6]); // mov rsi, rsp
    b.emit(&[0x31, 0xD2]); // xor edx, edx
    b.emit_syscall();

    b.emit_write_literal(b"[mach_demo server] RPC reply + child reaped PASS!\n");
    b.emit(&[0x48, 0x81, 0xC4, 0x80, 0x00, 0x00, 0x00]); // add rsp, 128
    b.emit_exit(0);

    // Child (Client) path
    let child_lbl = b.code.len();
    let d_child = (child_lbl as isize - (jz_child + 6) as isize) as i32;
    b.code[jz_child + 2..jz_child + 6].copy_from_slice(&d_child.to_le_bytes());

    // Sleep 20 ms
    b.emit_mov_eax(35);
    b.emit_mov_edi(20);
    b.emit_syscall();

    // r13 = SYS_BOOTSTRAP_LOOKUP(334)(svc_ptr, svc_len)
    b.emit_mov_eax(334);
    b.emit(&[0x48, 0x8D, 0x3D]);
    let p_lup = b.code.len();
    b.emit(&[0, 0, 0, 0]);
    b.lea_patches.push((p_lup, svc_off));
    b.emit(&[0xBE]);
    b.emit(&svc_len.to_le_bytes());
    b.emit_syscall();
    b.emit(&[0x49, 0x89, 0xC5]); // mov r13, rax

    // r14 = SYS_MACH_PORT_ALLOCATE (reply port)
    b.emit_mov_eax(330);
    b.emit_syscall();
    b.emit(&[0x49, 0x89, 0xC6]); // mov r14, rax

    // r15 = SYS_MMAP(0, 4096, 3) (OOL zero-copy page)
    b.emit_mov_eax(9);
    b.emit_mov_edi(0);
    b.emit(&[0xBE, 0x00, 0x10, 0x00, 0x00]);
    b.emit_mov_edx(3);
    b.emit_syscall();
    b.emit(&[0x49, 0x89, 0xC7]); // mov r15, rax

    // Write 0xCAFEBABE into OOL page
    b.emit(&[0x41, 0xC7, 0x07, 0xBE, 0xBA, 0xFE, 0xCA]); // mov dword ptr [r15], 0xCAFEBABE

    b.emit(&[0x48, 0x81, 0xEC, 0x80, 0x00, 0x00, 0x00]); // sub rsp, 128

    // Build complex MachMsgHeader + MachMsgOolDesc at [rsp..rsp+48]
    b.emit(&[0xC7, 0x04, 0x24, 19, 21, 0x00, 0x80]); // bits = COMPLEX | MAKE_SEND_ONCE<<8 | COPY_SEND
    b.emit(&[0xC7, 0x44, 0x24, 4, 48, 0, 0, 0]); // size = 48
    b.emit(&[0x44, 0x89, 0x6C, 0x24, 8]); // remote_port = r13d
    b.emit(&[0x44, 0x89, 0x74, 0x24, 12]); // local_port = r14d
    b.emit(&[0xC7, 0x44, 0x24, 16, 0x2B, 0x02, 0, 0]); // id = 555
    b.emit(&[0xC7, 0x44, 0x24, 20, 0, 0, 0, 0]); // reserved = 0
    b.emit(&[0x4C, 0x89, 0x7C, 0x24, 24]); // ool.addr = r15
    b.emit(&[0x48, 0xC7, 0x44, 0x24, 32, 0x00, 0x10, 0x00, 0x00]); // ool.size = 4096
    b.emit(&[0x48, 0xC7, 0x44, 0x24, 40, 0, 0, 0, 0]); // ool.deallocate + reserved = 0

    // Atomic RPC: SYS_MACH_MSG(rsp, SEND|RCV=3, send=48, rcv_cap=128, rcv_name=r14, timeout=0)
    b.emit_mov_eax(332);
    b.emit(&[0x48, 0x89, 0xE7]); // mov rdi, rsp
    b.emit(&[0xBE, 0x03, 0x00, 0x00, 0x00]); // mov esi, 3
    b.emit_mov_edx(48);
    b.emit(&[0x41, 0xBA, 0x80, 0x00, 0x00, 0x00]); // mov r10d, 128
    b.emit(&[0x4D, 0x89, 0xF0]); // mov r8, r14
    b.emit(&[0x45, 0x31, 0xC9]); // xor r9d, r9d
    b.emit_syscall();

    // Unmap OOL page in client
    b.emit_mov_eax(11);
    b.emit(&[0x4C, 0x89, 0xFF]); // mov rdi, r15
    b.emit(&[0xBE, 0x00, 0x10, 0x00, 0x00]); // mov esi, 4096
    b.emit_syscall();

    // Verify reply header.id == 777
    b.emit(&[0x81, 0x7C, 0x24, 16, 0x09, 0x03, 0, 0]); // cmp dword ptr [rsp + 16], 777
    let je_rep = b.code.len();
    b.emit(&[0x0F, 0x84, 0, 0, 0, 0]);
    b.emit_write_literal(b"[mach_demo child] Bad RPC reply id\n");
    b.emit(&[0x48, 0x81, 0xC4, 0x80, 0x00, 0x00, 0x00]);
    b.emit_exit(4);

    let rep_ok = b.code.len();
    let d_rep = (rep_ok as isize - (je_rep + 6) as isize) as i32;
    b.code[je_rep + 2..je_rep + 6].copy_from_slice(&d_rep.to_le_bytes());

    b.emit_write_literal(b"[mach_demo child] Received reply id=777 via Bootstrap RPC PASS!\n");
    b.emit(&[0x48, 0x81, 0xC4, 0x80, 0x00, 0x00, 0x00]); // add rsp, 128
    b.emit_exit(0);

    b.build()
}

/// FreeBSD ABI (`ELFOSABI_FREEBSD = 9`) verification ELF (`/freebsd_demo.elf`):
/// 1) Parses initial FreeBSD stack + Auxiliary Vector (`AT_PAGESZ = 6`)
/// 2) Verifies `RFLAGS.CF = 1` + positive `RAX = 9` (`FBSD_EBADF`) on `close(999)`
/// 3) Queries `hw.ncpu` (`[6, 3]`) and `kern.ostype` (`[1, 1]`) via `SYS_FBSD_SYSCTL (202)`
/// 4) Tests `kqueue` + `kevent` (`EVFILT_TIMER` 30ms & `EVFILT_PROC` `NOTE_EXIT` across `fork()`)
/// 5) Installs `SIGUSR1` handler via `sigaction (416)`, fires `kill (37)`, returns via `sigreturn (417)`
pub fn build_freebsd_demo_elf() -> Vec<u8> {
    let mut b = MiniElfBuilder::new_with_abi(super::freebsd::ELFOSABI_FREEBSD);

    let sig_flag_off = b.data.len();
    b.data.extend_from_slice(&0u64.to_le_bytes());

    let emit_fbsd_write_lit = |b: &mut MiniElfBuilder, msg: &[u8]| {
        let (off, len) = b.add_string(msg);
        b.emit_mov_eax(4); // SYS_FBSD_WRITE
        b.emit_mov_edi(1); // stdout
        b.emit_lea_rsi_data(off);
        b.emit_mov_edx(len);
        b.emit_syscall();
    };

    let emit_fbsd_print_rax_ln = |b: &mut MiniElfBuilder| {
        b.emit(&[0x48, 0x83, 0xEC, 32]); // sub rsp, 32
        b.emit(&[0x48, 0x8D, 0x74, 0x24, 32]); // lea rsi, [rsp + 32]
        b.emit(&[0x48, 0xFF, 0xCE]); // dec rsi
        b.emit(&[0xC6, 0x06, 0x0A]); // mov byte ptr [rsi], '\n'
        b.emit(&[0x41, 0xB8, 0x01, 0x00, 0x00, 0x00]); // mov r8d, 1
        b.emit(&[0xB9, 0x0A, 0x00, 0x00, 0x00]); // mov ecx, 10
        let loop_top = b.code.len();
        b.emit(&[0x31, 0xD2]); // xor edx, edx
        b.emit(&[0x48, 0xF7, 0xF1]); // div rcx
        b.emit(&[0x80, 0xC2, 0x30]); // add dl, '0'
        b.emit(&[0x48, 0xFF, 0xCE]); // dec rsi
        b.emit(&[0x88, 0x16]); // mov [rsi], dl
        b.emit(&[0x49, 0xFF, 0xC0]); // inc r8
        b.emit(&[0x48, 0x85, 0xC0]); // test rax, rax
        let next_ip = b.code.len() + 2;
        let rel8 = (loop_top as isize - next_ip as isize) as i8 as u8;
        b.emit(&[0x75, rel8]);
        b.emit(&[0x4C, 0x89, 0xC2]); // mov rdx, r8
        b.emit_mov_eax(4); // SYS_FBSD_WRITE
        b.emit_mov_edi(1);
        b.emit_syscall();
        b.emit(&[0x48, 0x83, 0xC4, 32]); // add rsp, 32
    };

    let emit_fbsd_exit = |b: &mut MiniElfBuilder, code: u32| {
        b.emit_mov_eax(1); // SYS_FBSD_EXIT
        b.emit_mov_edi(code);
        b.emit_syscall();
    };

    // Save FreeBSD initial stack pointer (rdi) into r15
    b.emit(&[0x49, 0x89, 0xFF]); // mov r15, rdi

    emit_fbsd_write_lit(&mut b, b"[freebsd_demo] ELFOSABI_FREEBSD (9) started, argc=");
    b.emit(&[0x49, 0x8B, 0x07]); // mov rax, [r15]
    emit_fbsd_print_rax_ln(&mut b);

    // Walk stack: rbx = r15 + 8 + (argc * 8) + 8(argv_null) + 8(envp_null) -> auxv
    b.emit(&[0x49, 0x8B, 0x07]); // mov rax, [r15] (argc)
    b.emit(&[0x49, 0x8D, 0x5C, 0xC7, 0x18]); // lea rbx, [r15 + rax*8 + 24]
    let aux_loop = b.code.len();
    b.emit(&[0x48, 0x8B, 0x03]); // mov rax, [rbx]
    b.emit(&[0x48, 0x85, 0xC0]); // test rax, rax (AT_NULL?)
    let jz_aux_done = b.code.len();
    b.emit(&[0x0F, 0x84, 0, 0, 0, 0]);
    b.emit(&[0x48, 0x83, 0xF8, 6]); // cmp rax, AT_PAGESZ (6)
    let je_pagesz = b.code.len();
    b.emit(&[0x0F, 0x84, 0, 0, 0, 0]);
    b.emit(&[0x48, 0x83, 0xC3, 16]); // add rbx, 16
    let j_aux = b.code.len() + 5;
    let rel_aux = (aux_loop as isize - j_aux as isize) as i32;
    b.emit(&[0xE9]);
    b.emit(&rel_aux.to_le_bytes());

    let pagesz_found = b.code.len();
    let d_ps = (pagesz_found as isize - (je_pagesz + 6) as isize) as i32;
    b.code[je_pagesz + 2..je_pagesz + 6].copy_from_slice(&d_ps.to_le_bytes());
    b.emit(&[0x4C, 0x8B, 0x73, 8]); // mov r14, [rbx + 8]
    emit_fbsd_write_lit(&mut b, b"[freebsd_demo] auxv[AT_PAGESZ]=");
    b.emit(&[0x4C, 0x89, 0xF0]); // mov rax, r14
    emit_fbsd_print_rax_ln(&mut b);

    let aux_done = b.code.len();
    let d_ad = (aux_done as isize - (jz_aux_done + 6) as isize) as i32;
    b.code[jz_aux_done + 2..jz_aux_done + 6].copy_from_slice(&d_ad.to_le_bytes());

    // 2) RFLAGS.CF + positive errno test: SYS_FBSD_CLOSE(999)
    b.emit_mov_eax(6); // SYS_FBSD_CLOSE
    b.emit_mov_edi(999);
    b.emit_syscall();
    let jc_ok = b.code.len();
    b.emit(&[0x0F, 0x82, 0, 0, 0, 0]); // jc cf_is_set
    emit_fbsd_write_lit(&mut b, b"[freebsd_demo] FAIL: expected CF=1 on close(999)\n");
    emit_fbsd_exit(&mut b, 1);

    let cf_is_set = b.code.len();
    let d_cf = (cf_is_set as isize - (jc_ok + 6) as isize) as i32;
    b.code[jc_ok + 2..jc_ok + 6].copy_from_slice(&d_cf.to_le_bytes());
    b.emit(&[0x48, 0x83, 0xF8, 9]); // cmp rax, 9 (FBSD_EBADF)
    let je_ebadf = b.code.len();
    b.emit(&[0x0F, 0x84, 0, 0, 0, 0]);
    emit_fbsd_write_lit(&mut b, b"[freebsd_demo] FAIL: errno != 9\n");
    emit_fbsd_exit(&mut b, 2);

    let ebadf_ok = b.code.len();
    let d_eb = (ebadf_ok as isize - (je_ebadf + 6) as isize) as i32;
    b.code[je_ebadf + 2..je_ebadf + 6].copy_from_slice(&d_eb.to_le_bytes());
    emit_fbsd_write_lit(&mut b, b"[freebsd_demo] RFLAGS.CF=1 & errno=9 (EBADF) PASS!\n");

    // 3) SYS_FBSD_SYSCTL (202): hw.ncpu [6, 3] & kern.ostype [1, 1]
    b.emit(&[0x48, 0x83, 0xEC, 64]); // sub rsp, 64
    b.emit(&[0xC7, 0x04, 0x24, 6, 0, 0, 0]); // [rsp] = CTL_HW (6)
    b.emit(&[0xC7, 0x44, 0x24, 4, 3, 0, 0, 0]); // [rsp+4] = HW_NCPU (3)
    b.emit(&[0x48, 0xC7, 0x44, 0x24, 8, 4, 0, 0, 0]); // [rsp+8] = oldlen (4)
    b.emit_mov_eax(202); // SYS_FBSD_SYSCTL
    b.emit(&[0x48, 0x89, 0xE7]); // rdi = &mib
    b.emit(&[0xBE, 2, 0, 0, 0]); // rsi = 2
    b.emit(&[0x48, 0x8D, 0x54, 0x24, 16]); // rdx = oldp ([rsp+16])
    b.emit(&[0x4C, 0x8D, 0x54, 0x24, 8]); // r10 = oldlenp ([rsp+8])
    b.emit(&[0x45, 0x31, 0xC0]); // xor r8d, r8d
    b.emit(&[0x45, 0x31, 0xC9]); // xor r9d, r9d
    b.emit_syscall();
    emit_fbsd_write_lit(&mut b, b"[freebsd_demo] sysctl hw.ncpu=");
    b.emit(&[0x8B, 0x44, 0x24, 16]); // mov eax, [rsp+16]
    emit_fbsd_print_rax_ln(&mut b);

    // kern.ostype [1, 1]
    b.emit(&[0xC7, 0x04, 0x24, 1, 0, 0, 0]); // CTL_KERN (1)
    b.emit(&[0xC7, 0x44, 0x24, 4, 1, 0, 0, 0]); // KERN_OSTYPE (1)
    b.emit(&[0x48, 0xC7, 0x44, 0x24, 8, 16, 0, 0, 0]); // oldlen = 16
    b.emit_mov_eax(202);
    b.emit(&[0x48, 0x89, 0xE7]);
    b.emit(&[0xBE, 2, 0, 0, 0]);
    b.emit(&[0x48, 0x8D, 0x54, 0x24, 16]);
    b.emit(&[0x4C, 0x8D, 0x54, 0x24, 8]);
    b.emit(&[0x45, 0x31, 0xC0]);
    b.emit(&[0x45, 0x31, 0xC9]);
    b.emit_syscall();
    emit_fbsd_write_lit(&mut b, b"[freebsd_demo] sysctl kern.ostype=");
    b.emit_mov_eax(4);
    b.emit_mov_edi(1);
    b.emit(&[0x48, 0x8D, 0x74, 0x24, 16]); // rsi = oldp
    b.emit_mov_edx(7); // "FreeBSD"
    b.emit_syscall();
    emit_fbsd_write_lit(&mut b, b"\n");
    b.emit(&[0x48, 0x83, 0xC4, 64]); // add rsp, 64

    // 4) kqueue (362) + EVFILT_TIMER + fork() + EVFILT_PROC
    b.emit_mov_eax(362); // SYS_FBSD_KQUEUE
    b.emit_syscall();
    b.emit(&[0x49, 0x89, 0xC4]); // mov r12, rax (kq_fd)

    b.emit(&[0x48, 0x83, 0xEC, 80]); // sub rsp, 80 (change@rsp, event@rsp+32, timespec@rsp+64)
    // Build KEvent at [rsp..rsp+32]: EVFILT_TIMER(-7 = 0xFFF9), EV_ADD|EV_ONESHOT(0x0011), data=25ms
    b.emit(&[0x48, 0xC7, 0x04, 0x24, 1, 0, 0, 0]); // ident = 1
    b.emit(&[0x66, 0xC7, 0x44, 0x24, 8, 0xF9, 0xFF]); // filter = -7 (EVFILT_TIMER)
    b.emit(&[0x66, 0xC7, 0x44, 0x24, 10, 0x11, 0x00]); // flags = EV_ADD | EV_ONESHOT
    b.emit(&[0xC7, 0x44, 0x24, 12, 0x02, 0, 0, 0]); // fflags = NOTE_MSECONDS
    b.emit(&[0x48, 0xC7, 0x44, 0x24, 16, 25, 0, 0, 0]); // data = 25 ms
    b.emit(&[0x48, 0xC7, 0x44, 0x24, 24, 0x34, 0x12, 0, 0]); // udata = 0x1234

    // SYS_FBSD_KEVENT(kq=r12, rsp, 1, rsp+32, 1, 0)
    b.emit_mov_eax(363);
    b.emit(&[0x4C, 0x89, 0xE7]); // rdi = r12
    b.emit(&[0x48, 0x89, 0xE6]); // rsi = rsp
    b.emit_mov_edx(1);
    b.emit(&[0x4C, 0x8D, 0x54, 0x24, 32]); // r10 = rsp+32
    b.emit(&[0x41, 0xB8, 1, 0, 0, 0]); // r8d = 1
    b.emit(&[0x45, 0x31, 0xC9]); // xor r9d, r9d
    b.emit_syscall();
    b.emit(&[0x48, 0x83, 0xF8, 1]); // cmp rax, 1
    let je_tmr = b.code.len();
    b.emit(&[0x0F, 0x84, 0, 0, 0, 0]);
    emit_fbsd_write_lit(&mut b, b"[freebsd_demo] FAIL: kqueue EVFILT_TIMER\n");
    b.emit(&[0x48, 0x83, 0xC4, 80]);
    emit_fbsd_exit(&mut b, 3);

    let tmr_ok = b.code.len();
    let d_tm = (tmr_ok as isize - (je_tmr + 6) as isize) as i32;
    b.code[je_tmr + 2..je_tmr + 6].copy_from_slice(&d_tm.to_le_bytes());
    emit_fbsd_write_lit(&mut b, b"[freebsd_demo] kqueue EVFILT_TIMER (25ms) fired PASS!\n");

    // fork() + EVFILT_PROC
    b.emit_mov_eax(2); // SYS_FBSD_FORK
    b.emit_syscall();
    b.emit(&[0x48, 0x85, 0xC0]); // test rax, rax
    let jz_child = b.code.len();
    b.emit(&[0x0F, 0x84, 0, 0, 0, 0]); // je child_proc

    // Parent path: r13 = child_pid
    b.emit(&[0x49, 0x89, 0xC5]); // mov r13, rax
    b.emit(&[0x4C, 0x89, 0x2C, 0x24]); // mov [rsp], r13 (ident = child_pid)
    b.emit(&[0x66, 0xC7, 0x44, 0x24, 8, 0xFB, 0xFF]); // filter = -5 (EVFILT_PROC)
    b.emit(&[0x66, 0xC7, 0x44, 0x24, 10, 0x11, 0x00]); // flags = EV_ADD | EV_ONESHOT
    b.emit(&[0xC7, 0x44, 0x24, 12, 0, 0, 0, 0x80]); // fflags = NOTE_EXIT (0x80000000)
    b.emit(&[0x48, 0xC7, 0x44, 0x24, 16, 0, 0, 0, 0]); // data = 0
    b.emit(&[0x48, 0xC7, 0x44, 0x24, 24, 0, 0, 0, 0]); // udata = 0

    b.emit_mov_eax(363); // SYS_FBSD_KEVENT
    b.emit(&[0x4C, 0x89, 0xE7]); // rdi = kq
    b.emit(&[0x48, 0x89, 0xE6]); // rsi = rsp
    b.emit_mov_edx(1);
    b.emit(&[0x4C, 0x8D, 0x54, 0x24, 32]); // r10 = rsp+32
    b.emit(&[0x41, 0xB8, 1, 0, 0, 0]); // r8d = 1
    b.emit(&[0x45, 0x31, 0xC9]); // xor r9d, r9d
    b.emit_syscall();

    emit_fbsd_write_lit(&mut b, b"[freebsd_demo parent] kqueue EVFILT_PROC NOTE_EXIT status=");
    b.emit(&[0x48, 0x8B, 0x44, 0x24, 48]); // mov rax, [rsp + 32 + 16] (event.data)
    emit_fbsd_print_rax_ln(&mut b);

    // Reap child via SYS_FBSD_WAIT4(r13, 0, 0, 0) & close kq
    b.emit_mov_eax(7);
    b.emit(&[0x4C, 0x89, 0xEF]); // rdi = r13
    b.emit(&[0x31, 0xF6]); // xor esi, esi
    b.emit_mov_edx(0);
    b.emit(&[0x45, 0x31, 0xD2]); // xor r10d, r10d
    b.emit_syscall();

    b.emit_mov_eax(6); // close(kq)
    b.emit(&[0x4C, 0x89, 0xE7]);
    b.emit_syscall();

    // 5) FreeBSD Signal test: sigaction(SIGUSR1=30, &act, NULL) + kill(pid, 30) + sigreturn(417)
    // Put handler address into [rsp..rsp+8], zero [rsp+8..rsp+32]
    b.emit(&[0x48, 0x8D, 0x05]); // lea rax, [rip + disp32] -> handler
    let h_disp = b.code.len();
    b.emit(&[0, 0, 0, 0]);
    b.emit(&[0x48, 0x89, 0x04, 0x24]); // mov [rsp], rax
    b.emit(&[0x48, 0xC7, 0x44, 0x24, 8, 0, 0, 0, 0]);
    b.emit(&[0x48, 0xC7, 0x44, 0x24, 16, 0, 0, 0, 0]);
    b.emit(&[0x48, 0xC7, 0x44, 0x24, 24, 0, 0, 0, 0]);

    b.emit_mov_eax(416); // SYS_FBSD_SIGACTION
    b.emit_mov_edi(30); // SIGUSR1
    b.emit(&[0x48, 0x89, 0xE6]); // rsi = rsp
    b.emit_mov_edx(0);
    b.emit_syscall();

    b.emit_mov_eax(20); // SYS_FBSD_GETPID
    b.emit_syscall();
    b.emit(&[0x48, 0x89, 0xC7]); // rdi = pid
    b.emit(&[0xBE, 30, 0, 0, 0]); // esi = SIGUSR1
    b.emit_mov_eax(37); // SYS_FBSD_KILL
    b.emit_syscall();

    // After return from SIGUSR1 handler via USER_SIGTRAMP_ADDR -> sigreturn(417), check [sig_flag] == 1
    b.emit(&[0x48, 0x8D, 0x05]); // lea rax, [rip + disp32]
    let f_disp_chk = b.code.len();
    b.emit(&[0, 0, 0, 0]);
    b.lea_patches.push((f_disp_chk, sig_flag_off));
    b.emit(&[0x48, 0x83, 0x38, 1]); // cmp qword ptr [rax], 1
    let je_sig = b.code.len();
    b.emit(&[0x0F, 0x84, 0, 0, 0, 0]);
    emit_fbsd_write_lit(&mut b, b"[freebsd_demo] FAIL: SIGUSR1 handler did not run!\n");
    b.emit(&[0x48, 0x83, 0xC4, 80]);
    emit_fbsd_exit(&mut b, 4);

    let sig_ok = b.code.len();
    let d_sig = (sig_ok as isize - (je_sig + 6) as isize) as i32;
    b.code[je_sig + 2..je_sig + 6].copy_from_slice(&d_sig.to_le_bytes());

    emit_fbsd_write_lit(&mut b, b"[freebsd_demo] sigaction(SIGUSR1) + kill + sigreturn PASS!\n");
    b.emit(&[0x48, 0x83, 0xC4, 80]);
    emit_fbsd_exit(&mut b, 0);

    // Child process body
    let child_lbl = b.code.len();
    let d_ch = (child_lbl as isize - (jz_child + 6) as isize) as i32;
    b.code[jz_child + 2..jz_child + 6].copy_from_slice(&d_ch.to_le_bytes());
    b.emit(&[0x48, 0xC7, 0x44, 0x24, 64, 0, 0, 0, 0]); // tv_sec = 0
    b.emit(&[0x48, 0xC7, 0x44, 0x24, 72, 0x00, 0x2D, 0x31, 0x01]); // tv_nsec = 20_000_000 (20ms)
    b.emit_mov_eax(240); // SYS_FBSD_NANOSLEEP
    b.emit(&[0x48, 0x8D, 0x7C, 0x24, 64]); // rdi = &timespec
    b.emit(&[0x31, 0xF6]); // xor esi, esi
    b.emit_syscall();
    emit_fbsd_write_lit(&mut b, b"[freebsd_demo child] exiting with code 77\n");
    b.emit(&[0x48, 0x83, 0xC4, 80]);
    emit_fbsd_exit(&mut b, 77);

    // SIGUSR1 handler function: sets sig_flag = 1 and returns (via USER_SIGTRAMP_ADDR)
    let handler_lbl = b.code.len();
    b.code_lea_patches.push((h_disp, handler_lbl));
    b.emit(&[0x48, 0x8D, 0x05]); // lea rax, [rip + disp32]
    let f_disp_set = b.code.len();
    b.emit(&[0, 0, 0, 0]);
    b.lea_patches.push((f_disp_set, sig_flag_off));
    b.emit(&[0x48, 0xC7, 0x00, 1, 0, 0, 0]); // mov qword ptr [rax], 1
    b.emit(&[0xC3]); // ret -> USER_SIGTRAMP_ADDR -> SYS_FBSD_SIGRETURN(417)

    b.build()
}



