export type AlertSeverity = 'Info' | 'Warning' | 'Error' | 'CriticalFault';
export type AlertSubsystem =
  | 'CpuRing3'
  | 'Memory'
  | 'StorageVfs'
  | 'Network'
  | 'Audio'
  | 'ElfLoader'
  | 'Desktop';

export interface FaultTelemetry {
  pid: number;
  vector: number;
  vectorName: string;
  errorCode: string;
  rip: string;
  rsp: string;
  cr2: string;
  syscalls: number;
  pmmOk: boolean;
}

export interface SystemAlert {
  id: number;
  timestamp: string;
  severity: AlertSeverity;
  subsystem: AlertSubsystem;
  title: string;
  summary: string;
  hint: string;
  codeTag: string;
  read: boolean;
  telemetry?: FaultTelemetry;
}

export interface Ring3Process {
  pid: number;
  name: string;
  path: string;
  state: 'Running' | 'Sleeping' | 'WaitingGui' | 'Exited';
  cr3: string;
  vruntimeMs: number;
  syscalls: number;
  memKb: number;
  kind: 'counter' | 'gui_plasma' | 'daemon' | 'oneshot';
  counterVal: number;
  hasWindow?: boolean;
}

export interface KThreadInfo {
  tid: number;
  name: string;
  cpuCore: number;
  state: 'Running' | 'Yielding' | 'Waiting';
  stackSizeKb: number;
  ticks: number;
}

export interface KernelDriverEntry {
  name: string;
  deviceClass: 'TIMER' | 'KEYBOARD' | 'MOUSE' | 'DISPLAY' | 'STORAGE' | 'NETWORK' | 'AUDIO' | 'CTRL';
  bus: string;
  status: 'ACTIVE' | 'REGISTERED';
  irq: string;
  details: string;
}

export interface NetState {
  nicName: string;
  linkUp: boolean;
  mac: string;
  ip: string;
  gateway: string;
  dns: string;
  subnet: string;
  rxPackets: number;
  txPackets: number;
  dhcpAcquired: boolean;
}

export interface KLogEntry {
  timeSec: string;
  subsystem: string;
  message: string;
}

class AnuKernel {
  readonly bootTime = Date.now();
  private nextPid = 101;
  private nextAlertId = 1;
  private clipboard = '';
  private processes: Ring3Process[] = [];
  private kthreads: KThreadInfo[] = [
    { tid: 1, name: 'knet_worker', cpuCore: 1, state: 'Yielding', stackSizeKb: 64, ticks: 1420 },
    { tid: 2, name: 'kaudio_mixer', cpuCore: 2, state: 'Yielding', stackSizeKb: 64, ticks: 980 },
    { tid: 3, name: 'kstorage_flush', cpuCore: 3, state: 'Waiting', stackSizeKb: 64, ticks: 315 },
    { tid: 4, name: 'kcompositor_vsync', cpuCore: 0, state: 'Running', stackSizeKb: 64, ticks: 4820 },
  ];
  private drivers: KernelDriverEntry[] = [
    { name: 'lapic_timer', deviceClass: 'TIMER', bus: 'SYSTEM', status: 'ACTIVE', irq: 'IRQ 0 (1000 Hz)', details: 'Per-CPU LAPIC + PIT calibration' },
    { name: 'ps2_kbd', deviceClass: 'KEYBOARD', bus: 'PS/2', status: 'ACTIVE', irq: 'IRQ 1 (IOAPIC GSI 1)', details: 'Scancode Set 1 + Alt/Ctrl/Super' },
    { name: 'ps2_intellimouse', deviceClass: 'MOUSE', bus: 'PS/2', status: 'ACTIVE', irq: 'IRQ 12 (IOAPIC GSI 12)', details: '4-byte IntelliMouse wheel & buttons' },
    { name: 'amaryllis_fb', deviceClass: 'DISPLAY', bus: 'LINEAR-FB', status: 'ACTIVE', irq: 'N/A', details: '1600x900 XRGB8888 double-buffered' },
    { name: 'virtio_blk', deviceClass: 'STORAGE', bus: 'PCI 00:04.0 1AF4:1001', status: 'ACTIVE', irq: 'IRQ 11 (GSI 11)', details: 'Split-ring DMA, 64 KiB multi-block' },
    { name: 'ahci_sata', deviceClass: 'STORAGE', bus: 'PCI 00:05.0 8086:2922', status: 'ACTIVE', irq: 'IRQ 11 (GSI 11)', details: 'AHCI 1.3 HBA, 32 command slots' },
    { name: 'intel_e1000', deviceClass: 'NETWORK', bus: 'PCI 00:03.0 8086:100E', status: 'ACTIVE', irq: 'IRQ 10 (GSI 10)', details: 'Intel 82540EM MMIO + TX/RX rings' },
    { name: 'intel_ac97', deviceClass: 'AUDIO', bus: 'PCI 00:06.0 8086:2415', status: 'ACTIVE', irq: 'IRQ 9 (GSI 9)', details: 'BDL DMA mixer, 44.1 kHz VRA stereo' },
    { name: 'acpi_pm', deviceClass: 'CTRL', bus: 'PCI 00:01.3 8086:7113', status: 'ACTIVE', irq: 'SCI 9', details: 'FADT/MADT/MCFG/HPET parsed' },
  ];
  private net: NetState = {
    nicName: 'Intel 82540EM (e1000)',
    linkUp: true,
    mac: '52:54:00:12:34:56',
    ip: '10.0.2.15',
    gateway: '10.0.2.2',
    dns: '10.0.2.3',
    subnet: '255.255.255.0',
    rxPackets: 128,
    txPackets: 94,
    dhcpAcquired: true,
  };
  private klog: KLogEntry[] = [
    { timeSec: '0.000', subsystem: 'boot', message: 'ANU Kernel v0.11 entered (x86_64)' },
    { timeSec: '0.014', subsystem: 'pmm', message: 'Bootstrap & Dynamic PMM verified (262144 frames / 1024 MiB)' },
    { timeSec: '0.029', subsystem: 'vmm', message: '4-level paging active (NXE + W^X enforced)' },
    { timeSec: '0.044', subsystem: 'smp', message: '4 LAPIC cores online via INIT-SIPI-SIPI trampoline 0x8000' },
    { timeSec: '0.062', subsystem: 'iokit', message: 'IORegistry matched 9 kernel drivers & WorkLoops' },
    { timeSec: '0.088', subsystem: 'net', message: 'e1000 link UP — DHCP lease 10.0.2.15/24 gw 10.0.2.2' },
    { timeSec: '0.109', subsystem: 'userland', message: 'SYSCALL/SYSRET MSRs armed; ELF demos ready in /' },
  ];
  private alerts: SystemAlert[] = [];
  private activeModalAlert: SystemAlert | null = null;
  private listeners = new Set<() => void>();
  private tickHandle: number | null = null;
  private onOpenPlasmaWindow?: (pid: number) => void;

  constructor() {
    this.startSchedulerLoop();
  }

  setPlasmaLauncher(cb: (pid: number) => void) {
    this.onOpenPlasmaWindow = cb;
  }

  private startSchedulerLoop() {
    if (typeof window === 'undefined' || this.tickHandle !== null) return;
    this.tickHandle = window.setInterval(() => {
      let changed = false;
      for (const proc of this.processes) {
        if (proc.state === 'Running' || proc.state === 'WaitingGui') {
          proc.vruntimeMs += 250;
          proc.syscalls += proc.kind === 'gui_plasma' ? 15 : 2;
          if (proc.kind === 'counter') {
            proc.counterVal += 1;
          }
          changed = true;
        }
      }
      for (const kt of this.kthreads) {
        kt.ticks += kt.state === 'Running' ? 25 : 4;
      }
      if (changed) this.emit();
    }, 500);
  }

  subscribe(fn: () => void): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  private emit() {
    this.listeners.forEach((fn) => fn());
  }

  getClipboard(): string {
    return this.clipboard;
  }

  setClipboard(text: string) {
    this.clipboard = text;
  }

  uptimeSeconds(): number {
    return Math.floor((Date.now() - this.bootTime) / 1000);
  }

  getProcesses(): Ring3Process[] {
    return [...this.processes];
  }

  getKThreads(): KThreadInfo[] {
    return [...this.kthreads];
  }

  getDrivers(): KernelDriverEntry[] {
    return [...this.drivers];
  }

  getNetState(): NetState {
    return { ...this.net };
  }

  bumpNetTraffic(tx: number, rx: number) {
    this.net.txPackets += tx;
    this.net.rxPackets += rx;
    this.emit();
  }

  acquireDhcp(): NetState {
    this.net.dhcpAcquired = true;
    this.net.ip = '10.0.2.15';
    this.net.gateway = '10.0.2.2';
    this.net.dns = '10.0.2.3';
    this.net.txPackets += 2;
    this.net.rxPackets += 2;
    this.log('net', 'DHCPv4 ACK: leased 10.0.2.15/24 (GW 10.0.2.2, DNS 10.0.2.3)');
    this.emit();
    return { ...this.net };
  }

  getKLog(): KLogEntry[] {
    return [...this.klog];
  }

  clearKLog() {
    this.klog = [];
    this.emit();
  }

  log(subsystem: string, message: string) {
    const t = ((Date.now() - this.bootTime) / 1000).toFixed(3);
    this.klog.push({ timeSec: t, subsystem, message });
    if (this.klog.length > 100) this.klog.shift();
    this.emit();
  }

  getAlerts(): SystemAlert[] {
    return [...this.alerts];
  }

  getUnreadAlertCount(): number {
    return this.alerts.filter((a) => !a.read).length;
  }

  getActiveModalAlert(): SystemAlert | null {
    return this.activeModalAlert;
  }

  dismissModalAlert() {
    if (this.activeModalAlert) {
      const target = this.alerts.find((a) => a.id === this.activeModalAlert!.id);
      if (target) target.read = true;
      this.activeModalAlert = null;
      this.emit();
    }
  }

  openLatestAlert() {
    const latest = this.alerts[this.alerts.length - 1] ?? null;
    if (latest) {
      latest.read = true;
      this.activeModalAlert = latest;
      this.emit();
    }
  }

  reportAlert(
    alert: Omit<SystemAlert, 'id' | 'timestamp' | 'read'>,
    showModal = true
  ): SystemAlert {
    const entry: SystemAlert = {
      ...alert,
      id: this.nextAlertId++,
      timestamp: new Date().toLocaleTimeString(),
      read: showModal,
    };
    this.alerts.push(entry);
    if (this.alerts.length > 32) this.alerts.shift();
    if (showModal) {
      this.activeModalAlert = entry;
    }
    this.emit();
    return entry;
  }

  spawnBackgroundElf(path: string): Ring3Process {
    const pid = this.nextPid++;
    const name = path.split('/').pop() || path;
    const isGui = path.includes('gui_demo');
    const proc: Ring3Process = {
      pid,
      name,
      path,
      state: isGui ? 'WaitingGui' : 'Running',
      cr3: `0x${(0x1a00000 + pid * 0x4000).toString(16)}`,
      vruntimeMs: 0,
      syscalls: 4,
      memKb: isGui ? 512 : 128,
      kind: isGui ? 'gui_plasma' : 'counter',
      counterVal: 1,
      hasWindow: isGui,
    };
    this.processes.push(proc);
    this.log('userland', `SYS_SPAWN_ELF pid=${pid} (${path}) CR3=${proc.cr3}`);
    if (isGui && this.onOpenPlasmaWindow) {
      this.onOpenPlasmaWindow(pid);
    }
    this.emit();
    return proc;
  }

  killProcess(pid: number): boolean {
    const idx = this.processes.findIndex((p) => p.pid === pid);
    if (idx === -1) return false;
    const [removed] = this.processes.splice(idx, 1);
    this.log('userland', `killr3 pid=${pid} (${removed.name}) — reclaimed address space & frames`);
    this.emit();
    return true;
  }

  triggerFaultElf(path: string): SystemAlert {
    const pid = this.nextPid++;
    const isGuard = path.includes('guard');
    const cr2 = isGuard ? '0x00007fffffffeff8' : '0x0000000000000000';
    const rip = isGuard ? '0x0000000000400142' : '0x00000000004000b8';
    this.log(
      'trap',
      `Ring 3 #PF trapped in PID=${pid} (${path}) CR2=${cr2} RIP=${rip} — kernel intact`
    );
    return this.reportAlert(
      {
        severity: 'CriticalFault',
        subsystem: 'CpuRing3',
        title: isGuard ? 'Ring 3 Stack Guard Page Fault (#PF)' : 'Ring 3 Page Fault (#PF)',
        summary: isGuard
          ? `Process PID ${pid} (${path}) hit the 4 KiB unmapped stack guard page and was safely terminated.`
          : `Process PID ${pid} (${path}) dereferenced an unmapped virtual address (NULL page) at RIP=${rip}.`,
        hint: 'Ring 3 isolation trapped #PF via IDT -> CR3 & PMM frames reclaimed cleanly without kernel panic.',
        codeTag: 'EXC #PF (vec 14)',
        telemetry: {
          pid,
          vector: 14,
          vectorName: '#PF Page Fault',
          errorCode: '0x00000006 (User | Write | NotPresent)',
          rip,
          rsp: '0x00007ffffffffe80',
          cr2,
          syscalls: 3,
          pmmOk: true,
        },
      },
      true
    );
  }
}

export const anuKernel = new AnuKernel();
