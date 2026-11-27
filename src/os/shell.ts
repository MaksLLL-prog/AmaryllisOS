import { ac97Mixer } from './audio';
import { anuKernel } from './kernel';
import { vfs } from './vfs';

export interface ShellSpan {
  text: string;
  color: string;
}

export interface ShellResult {
  spans: ShellSpan[];
  clear?: boolean;
  action?:
    | { type: 'launch_gui' }
    | { type: 'exit_gui' }
    | { type: 'open_editor'; path: string }
    | { type: 'open_plasma'; pid: number };
}

const WHITE = '#EAEFF7';
const GREEN = '#28B478';
const GRAY = '#8C94A4';
const YELLOW = '#DCBE5A';
const RED = '#DC5A5A';
const CYAN = '#3890F0';

function line(text: string, color = WHITE): ShellSpan {
  return { text: text + '\n', color };
}

export async function executeShellCommand(rawInput: string): Promise<ShellResult> {
  const trimmed = rawInput.trim();
  if (!trimmed) return { spans: [] };

  const parts = trimmed.split(/\s+/);
  const cmd = parts[0];
  const args = parts.slice(1);
  const rest = trimmed.slice(cmd.length).trim();

  const out: ShellSpan[] = [];

  switch (cmd) {
    case 'clear':
      return { spans: [], clear: true };

    case 'gui':
    case 'startx':
      return {
        spans: [line('Launching Amaryllis Desktop Environment (Compositor)...', GREEN)],
        action: { type: 'launch_gui' },
      };

    case 'exit':
      return {
        spans: [line('Exiting Amaryllis Desktop to VGA/FB text shell...', YELLOW)],
        action: { type: 'exit_gui' },
      };

    case 'help':
      out.push(line('Available commands:', YELLOW));
      out.push(line('  help               - Show commands'));
      out.push(line('  clear              - Clear screen'));
      out.push(line('  about | version    - About AmaryllisOS & ANU kernel'));
      out.push(line('  echo <text>        - Print text'));
      out.push(line('  whoami | neofetch  - User and system information'));
      out.push(line('  uptime             - Show system uptime'));
      out.push(line('  heap [stress]      - Heap memory diagnostics'));
      out.push(line('  pmm [test|stress]  - Physical memory diagnostics'));
      out.push(line('  tasks | processes  - Kernel tasks & processes'));
      out.push(line('\nPCI, Drivers & Platform:', YELLOW));
      out.push(line('  lspci [-v|--drivers|--count] - PCI bus device inventory'));
      out.push(line('  drivers | ioreg    - Unified Driver Registry & IORegistry tree'));
      out.push(line('  acpi | apic | smp  - ACPI tables, LAPIC timer & SMP cores'));
      out.push(line('  klog | dmesg       - Kernel ring log buffer'));
      out.push(line('  kthread            - Preemptive kernel worker threads'));
      out.push(line('\nFilesystem & Storage:', YELLOW));
      out.push(line('  ls [path]          - List directory contents'));
      out.push(line('  cd [path] | pwd    - Change or print working directory'));
      out.push(line('  cat <path>         - Read file contents'));
      out.push(line('  write <path> <txt> - Create or overwrite file'));
      out.push(line('  mkdir <path> | rm <path> - Create or remove dir/file'));
      out.push(line('  diskinfo | sync    - Storage mounts, Page Cache & flush'));
      out.push(line('\nNetwork & Audio:', YELLOW));
      out.push(line('  netinfo | dhcp     - Intel e1000 & IPv4 DHCP lease'));
      out.push(line('  ping <host>        - DNS lookup & ICMP echo test'));
      out.push(line('  httpget <url>      - Real HTTP GET request'));
      out.push(line('  soundinfo | beep [freq] [ms] | play [track]'));
      out.push(line('\nUserland & ELF (Ring 3):', YELLOW));
      out.push(line('  mkelf | elfinfo <path> - Stage or inspect ELF64 binaries'));
      out.push(line('  exec <path> | execbg <path> - Run Ring 3 ELF binary'));
      out.push(line('  ps | killr3 <pid>  - List or kill Ring 3 processes'));
      out.push(line('  gui | startx | exit - Toggle Amaryllis Desktop / Text Shell', CYAN));
      break;

    case 'about':
      out.push(line('AmaryllisOS is a bare-metal x86_64 operating system.', WHITE));
      out.push(line('Kernel: ANU (Amaryllis is Not Unix) v0.11 — written in Rust.', GRAY));
      break;

    case 'version':
      out.push(line('AmaryllisOS version 0.11.0 (ANU Hybrid Kernel)', GREEN));
      out.push(line('Architecture: x86_64', GRAY));
      break;

    case 'whoami':
      out.push(line('root', GREEN));
      break;

    case 'echo':
      out.push(line(rest, WHITE));
      break;

    case 'uptime':
      out.push(
        { text: 'System uptime: ', color: GREEN },
        { text: `${anuKernel.uptimeSeconds()} seconds\n`, color: WHITE }
      );
      break;

    case 'neofetch':
      out.push(line(''));
      out.push({ text: '       /\\        ', color: GREEN }, line('AmaryllisOS', WHITE));
      out.push({ text: '      /  \\       ', color: GREEN }, line('System: AmaryllisOS v0.11', WHITE));
      out.push({ text: '     / /\\ \\      ', color: GREEN }, line('Kernel: ANU (Amaryllis is Not Unix)', WHITE));
      out.push({ text: '    / ____ \\     ', color: GREEN }, line('Architecture: x86_64 (4x SMP)', WHITE));
      out.push({ text: '   /_/    \\_\\    ', color: GREEN }, line('Shell: amsh', WHITE));
      out.push({ text: '                  ', color: WHITE }, line('User: root', WHITE));
      out.push({ text: '                  ', color: WHITE }, line(`Uptime: ${anuKernel.uptimeSeconds()}s`, WHITE));
      out.push({ text: '                  ', color: GRAY }, line('Compositor: Amaryllis Desktop (active)', GRAY));
      out.push(line(''));
      break;

    case 'pwd':
      out.push(line(vfs.getCwd(), WHITE));
      break;

    case 'cd': {
      const target = args[0] ?? '/';
      const res = vfs.setCwd(target);
      if (!res.ok) out.push(line(res.error, RED));
      break;
    }

    case 'ls': {
      const target = args[0] ?? vfs.getCwd();
      try {
        const entries = vfs.listDir(target);
        if (entries.length === 0) {
          out.push(line('(empty directory)', GRAY));
        } else {
          for (const e of entries) {
            const tag = e.kind === 'directory' ? '<DIR>  ' : `${String(e.size).padStart(6)}B`;
            out.push(
              { text: `${tag}  `, color: e.kind === 'directory' ? YELLOW : GRAY },
              {
                text: `${e.name}${e.kind === 'directory' ? '/' : ''}\n`,
                color: e.kind === 'directory' ? CYAN : e.name.endsWith('.elf') ? GREEN : WHITE,
              }
            );
          }
        }
      } catch (err) {
        out.push(line(err instanceof Error ? err.message : 'ls error', RED));
      }
      break;
    }

    case 'cat': {
      if (!args[0]) {
        out.push(line('Usage: cat <path>', YELLOW));
        break;
      }
      try {
        const content = vfs.readFile(args[0]);
        out.push(line(content.endsWith('\n') ? content.slice(0, -1) : content, WHITE));
      } catch (err) {
        out.push(line(err instanceof Error ? err.message : 'cat failed', RED));
      }
      break;
    }

    case 'write': {
      if (args.length < 2) {
        out.push(line('Usage: write <path> <text>', YELLOW));
        break;
      }
      const target = args[0];
      const body = rest.slice(target.length).trim();
      try {
        vfs.writeFile(target, body + '\n');
        out.push(line(`Wrote ${body.length + 1} bytes to ${vfs.resolvePath(target)}`, GREEN));
      } catch (err) {
        out.push(line(err instanceof Error ? err.message : 'write failed', RED));
      }
      break;
    }

    case 'mkdir': {
      if (!args[0]) {
        out.push(line('Usage: mkdir <path>', YELLOW));
        break;
      }
      try {
        vfs.createDir(args[0]);
        out.push(line(`Created directory ${vfs.resolvePath(args[0])}`, GREEN));
      } catch (err) {
        out.push(line(err instanceof Error ? err.message : 'mkdir failed', RED));
      }
      break;
    }

    case 'rm': {
      if (!args[0]) {
        out.push(line('Usage: rm <path>', YELLOW));
        break;
      }
      try {
        vfs.remove(args[0]);
        out.push(line(`Removed ${vfs.resolvePath(args[0])}`, GREEN));
      } catch (err) {
        out.push(line(err instanceof Error ? err.message : 'rm failed', RED));
      }
      break;
    }

    case 'diskinfo': {
      out.push(line('VFS Mounts & Unified Page Cache:', GREEN));
      for (const vol of vfs.volumeStats()) {
        out.push(
          line(
            `  ${vol.mount.padEnd(6)} ${vol.fsType.padEnd(6)} ${Math.round(vol.usedBytes / 1024)} KiB / ${Math.round(vol.capacityBytes / (1024 * 1024))} MiB  Cache Hit: ${vol.cacheHitRate}  [${vol.backend}]`,
            WHITE
          )
        );
      }
      break;
    }

    case 'sync':
      out.push(line('Flushed dirty 4 KiB Block/Page Cache pages to VirtIO-Blk DMA.', GREEN));
      break;

    case 'mount':
      out.push(line('Mounted / (RAMFS) and /disk (FAT32 over VirtIO-Blk DMA).', GREEN));
      break;

    case 'heap':
      if (args[0] === 'stress') {
        out.push(line('Running 64 MiB kernel heap allocation stress test... [PASS]', GREEN));
      } else {
        out.push(line('Kernel Heap Allocator: 64 MiB total | 11.4 MiB used | 52.6 MiB free', WHITE));
      }
      break;

    case 'pmm':
      if (args[0] === 'test' || args[0] === 'stress') {
        out.push(line('PMM Bitmap & Dynamic Frame Allocator test: 4096 frames alloc/free [PASS]', GREEN));
      } else {
        out.push(line('PMM: 262,144 physical frames (1024 MiB) | 18,432 used | 243,712 free', WHITE));
      }
      break;

    case 'lspci': {
      const drivers = anuKernel.getDrivers().filter((d) => d.bus.startsWith('PCI'));
      if (args[0] === '--count') {
        out.push(line(`PCI devices: ${drivers.length}`, GREEN));
      } else {
        out.push(line('PCI Device Manager:', GREEN));
        for (const d of drivers) {
          out.push(line(`  ${d.bus.padEnd(24)} ${d.name.padEnd(16)} [${d.status}] ${args[0] === '-v' ? d.details : ''}`, WHITE));
        }
      }
      break;
    }

    case 'drivers':
      out.push(line('Unified Kernel Driver Registry:', GREEN));
      for (const d of anuKernel.getDrivers()) {
        out.push(line(`  [${d.status}] ${d.deviceClass.padEnd(9)} ${d.name.padEnd(18)} ${d.bus}`, WHITE));
      }
      break;

    case 'ioreg':
      out.push(line('IORegistry Device Tree (IOKit-inspired):', GREEN));
      out.push(line('+- Root (x86_64 ACPI)', WHITE));
      for (const d of anuKernel.getDrivers()) {
        out.push(line(`   +- ${d.name} <${d.deviceClass}> [${d.irq}]`, GRAY));
      }
      break;

    case 'acpi':
      out.push(line('ACPI Tables: RSDP v2, XSDT, FADT (PM reboot/shutdown), MADT (4 LAPICs), MCFG, HPET', WHITE));
      break;

    case 'apic':
    case 'smp':
      out.push(line('SMP Topology: 4x x86_64 cores active (BSP #0 + AP #1..#3), LAPIC @ 1000 Hz, IPI 0xF0/0xF1', WHITE));
      break;

    case 'kthread':
      out.push(line('Cooperative Kernel Worker Threads (64 KiB + 4 KiB guard stacks):', GREEN));
      for (const kt of anuKernel.getKThreads()) {
        out.push(line(`  TID ${kt.tid}  ${kt.name.padEnd(18)} CPU#${kt.cpuCore}  ${kt.state.padEnd(9)} ticks=${kt.ticks}`, WHITE));
      }
      break;

    case 'klog':
    case 'dmesg':
      if (args[0] === 'clear') {
        anuKernel.clearKLog();
        out.push(line('Cleared klog ring buffer.', GRAY));
      } else {
        for (const entry of anuKernel.getKLog()) {
          out.push(
            { text: `[${entry.timeSec.padStart(7)}] `, color: GRAY },
            { text: `${entry.subsystem}: `, color: GREEN },
            line(entry.message, WHITE)
          );
        }
      }
      break;

    case 'ipc':
      out.push(line('IPC Subsystem: POSIX Pipes (16 KiB ring), Zero-Copy SHM Surfaces, SWAPGS per-CPU active', WHITE));
      break;

    case 'netinfo': {
      const n = anuKernel.getNetState();
      out.push(line(`NIC: ${n.nicName} (${n.linkUp ? 'Link UP' : 'DOWN'})`, GREEN));
      out.push(line(`  MAC: ${n.mac}  IPv4: ${n.ip}  GW: ${n.gateway}  DNS: ${n.dns}`, WHITE));
      out.push(line(`  RX: ${n.rxPackets} pkts  TX: ${n.txPackets} pkts`, GRAY));
      break;
    }

    case 'dhcp': {
      const n = anuKernel.acquireDhcp();
      out.push(line(`DHCP OK: IP ${n.ip}  GW ${n.gateway}  DNS ${n.dns}`, GREEN));
      break;
    }

    case 'ping': {
      const host = args[0] ?? 'example.com';
      out.push(line(`PING ${host} via Intel e1000...`, GRAY));
      try {
        const res = await fetch(`/api/net/dns?host=${encodeURIComponent(host)}`);
        const data = (await res.json()) as { ok: boolean; addresses?: Array<{ address: string }>; elapsedMs?: number; error?: string };
        anuKernel.bumpNetTraffic(4, 4);
        if (data.ok && data.addresses?.length) {
          const ip = data.addresses[0].address;
          for (let seq = 1; seq <= 4; seq++) {
            out.push(line(`64 bytes from ${ip} (${host}): icmp_seq=${seq} ttl=64 time=${data.elapsedMs ?? 12}ms`, GREEN));
          }
        } else {
          out.push(line(`Ping lookup error: ${data.error ?? 'unreachable'}`, RED));
        }
      } catch {
        out.push(line('Network request failed.', RED));
      }
      break;
    }

    case 'httpget': {
      const url = args[0] ?? 'https://example.com';
      out.push(line(`HTTP GET ${url}...`, GRAY));
      try {
        const res = await fetch(`/api/net/httpget?url=${encodeURIComponent(url)}`);
        const data = (await res.json()) as {
          ok: boolean;
          status?: number;
          statusText?: string;
          bytes?: number;
          elapsedMs?: number;
          body?: string;
          error?: string;
        };
        anuKernel.bumpNetTraffic(6, 8);
        if (data.status) {
          out.push(line(`HTTP ${data.status} ${data.statusText ?? ''} (${data.bytes ?? 0} bytes, ${data.elapsedMs ?? 0}ms)`, GREEN));
          const preview = (data.body ?? '').slice(0, 600);
          out.push(line(preview, WHITE));
        } else {
          out.push(line(`HTTP error: ${data.error ?? 'failed'}`, RED));
        }
      } catch {
        out.push(line('HTTP request failed.', RED));
      }
      break;
    }

    case 'soundinfo': {
      const st = ac97Mixer.getState();
      out.push(line(`Intel AC'97 Audio Driver: ACTIVE (Volume ${st.volume}%, ${st.muted ? 'Muted' : 'Unmuted'})`, GREEN));
      break;
    }

    case 'beep': {
      const freq = Number(args[0]) || 587;
      const ms = Number(args[1]) || 120;
      ac97Mixer.beep(freq, ms);
      out.push(line(`Played AC'97 tone: ${freq} Hz (${ms} ms)`, GREEN));
      break;
    }

    case 'play': {
      ac97Mixer.playTrack({
        id: 'startup_chime',
        title: args[0] || 'Startup Chime',
        subtitle: 'AC97 Stereo Synth',
        durationSec: 2.4,
        kind: 'builtin',
        builtinId: 'startup_chime',
      });
      out.push(line(`Streaming audio via AC'97 BDL DMA: ${args[0] || 'Startup Chime'}`, GREEN));
      break;
    }

    case 'mkelf':
      vfs.stageElfs();
      out.push(line('Staged Ring 3 ELF64 binaries in / (/hello.elf, /counter.elf, /gui_demo.elf, /net_demo.elf, /fault.elf, ...)', GREEN));
      break;

    case 'elfinfo': {
      const p = args[0];
      if (!p) {
        out.push(line('Usage: elfinfo <path.elf>', YELLOW));
        break;
      }
      const node = vfs.getNode(vfs.resolvePath(p));
      if (!node || !node.elfMeta) {
        out.push(line(`Not a valid ELF64 file: ${p}`, RED));
        break;
      }
      out.push(line(`ELF64 Header (${vfs.resolvePath(p)}):`, GREEN));
      out.push(line(`  Type: ${node.elfMeta.type}  Machine: ${node.elfMeta.machine}  Entry: ${node.elfMeta.entry}`, WHITE));
      for (const s of node.elfMeta.segments) {
        out.push(line(`  PT_LOAD vaddr=${s.vaddr} memsz=${s.memsz} flags=${s.flags}`, GRAY));
      }
      break;
    }

    case 'exec':
    case 'run':
    case 'execbg': {
      const target = args[0];
      if (!target) {
        out.push(line(`Usage: ${cmd} <path.elf>`, YELLOW));
        break;
      }
      const abs = vfs.resolvePath(target);
      const node = vfs.getNode(abs);
      if (!node) {
        out.push(line(`ELF not found: ${abs}`, RED));
        break;
      }
      if (abs.includes('fault')) {
        anuKernel.triggerFaultElf(abs);
        out.push(line(`[#PF] Ring 3 exception trapped cleanly in ${abs}; Fault Inspector opened.`, RED));
        break;
      }
      if (abs.includes('hello')) {
        out.push(line('[Ring 3 stdout] Hello from AmaryllisOS Userland (SYSCALL/SYSRET)!', GREEN));
        out.push(line('Process exited cleanly with code 0.', GRAY));
        break;
      }
      if (abs.includes('echo')) {
        out.push(line(`[Ring 3 echo.elf] argv = [${[abs, ...args.slice(1)].join(', ')}]`, GREEN));
        break;
      }
      if (abs.includes('lazy_demo')) {
        out.push(line('[Ring 3 lazy_demo.elf] SYS_MMAP 64 KiB -> Demand paging mapped 4 frames on access [OK]', GREEN));
        break;
      }
      if (abs.includes('net_demo')) {
        const res = await fetch('/api/net/dns?host=example.com');
        const data = (await res.json()) as { ok: boolean; addresses?: Array<{ address: string }> };
        const ip = data.addresses?.[0]?.address ?? '93.184.216.34';
        out.push(line(`[Ring 3 net_demo.elf] SYS_DNS_RESOLVE("example.com") -> ${ip}`, GREEN));
        out.push(line(`[Ring 3 net_demo.elf] SYS_NET_PING(${ip}) -> 14 ms [OK]`, GREEN));
        break;
      }
      const proc = anuKernel.spawnBackgroundElf(abs);
      out.push(line(`Spawned Ring 3 PID=${proc.pid} (${abs}) CR3=${proc.cr3}`, GREEN));
      if (proc.kind === 'gui_plasma') {
        return { spans: out, action: { type: 'open_plasma', pid: proc.pid } };
      }
      break;
    }

    case 'ps':
    case 'tasks':
    case 'processes': {
      const procs = anuKernel.getProcesses();
      if (procs.length === 0) {
        out.push(line('No active Ring 3 processes. Try: execbg /counter.elf', GRAY));
      } else {
        out.push(line('PID   STATE        CR3         VRUNTIME   SYSCALLS  NAME', GREEN));
        for (const p of procs) {
          out.push(
            line(
              `${String(p.pid).padEnd(5)} ${p.state.padEnd(12)} ${p.cr3}  ${String(p.vruntimeMs + 'ms').padEnd(10)} ${String(p.syscalls).padEnd(9)} ${p.path}`,
              WHITE
            )
          );
        }
      }
      break;
    }

    case 'killr3':
    case 'prockill': {
      const pid = Number(args[0]);
      if (!pid) {
        out.push(line('Usage: killr3 <pid>', YELLOW));
        break;
      }
      if (anuKernel.killProcess(pid)) {
        out.push(line(`Terminated Ring 3 PID ${pid} and reclaimed memory.`, GREEN));
      } else {
        out.push(line(`No active process with PID ${pid}.`, RED));
      }
      break;
    }

    default:
      out.push(
        { text: 'Unknown command: ', color: RED },
        line(trimmed, WHITE),
        line("Type 'help' for available commands.", GRAY)
      );
      break;
  }

  return { spans: out };
}
