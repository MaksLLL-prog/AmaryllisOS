export type VfsNodeKind = 'file' | 'directory';

export interface VfsNode {
  name: string;
  kind: VfsNodeKind;
  content?: string;
  children?: Map<string, VfsNode>;
  readOnly?: boolean;
  fsLabel?: 'RAMFS' | 'FAT32';
  modifiedAt: number;
  elfMeta?: {
    entry: string;
    machine: string;
    type: string;
    segments: Array<{ vaddr: string; memsz: number; flags: string }>;
    description: string;
  };
}

export interface VfsDirEntry {
  name: string;
  path: string;
  kind: VfsNodeKind;
  size: number;
  readOnly: boolean;
  fsLabel: 'RAMFS' | 'FAT32';
  modifiedAt: number;
}

export interface VfsVolumeInfo {
  mount: string;
  fsType: 'RAMFS' | 'FAT32';
  backend: string;
  readOnly: boolean;
  capacityBytes: number;
  usedBytes: number;
  cacheHitRate: string;
}

function makeFile(
  name: string,
  content: string,
  fsLabel: 'RAMFS' | 'FAT32' = 'RAMFS',
  elfMeta?: VfsNode['elfMeta']
): VfsNode {
  return {
    name,
    kind: 'file',
    content,
    fsLabel,
    modifiedAt: Date.now(),
    elfMeta,
  };
}

function makeDir(name: string, fsLabel: 'RAMFS' | 'FAT32' = 'RAMFS'): VfsNode {
  return {
    name,
    kind: 'directory',
    children: new Map(),
    fsLabel,
    modifiedAt: Date.now(),
  };
}

const README_TXT = `Welcome to AmaryllisOS!

Root '/' uses writable RAMFS.
Physical IDE / VirtIO / AHCI disks (FAT32) are mounted at '/disk'.
External Rust apps are supported via 'amaryllis-sdk/'.

Ring 3 Demos (double-click in Files or run in Terminal):
  /hello.elf        Minimal Ring 3 console output
  /counter.elf      Preemptive multitasking worker
  /gui_demo.elf     External Ring 3 GUI window (SDK Plasma Wave)
  /net_demo.elf     Ring 3 Ping & DNS syscall test
  /fault.elf        Ring 3 #PF trap & Fault Inspector recovery test
  /echo.elf         SysV ABI argc/argv echo demo
  /lazy_demo.elf    Demand-paged SYS_MMAP lazy allocation demo
  /guard_fault.elf  Stack guard page protection demo

Shortcuts:
  Alt+Space  Spotlight Launcher
  Ctrl+S     Save in Text Editor
  Ctrl+C/V   Copy / Paste across Editor & Terminal
`;

const NOTES_TXT = `Amaryllis Notes
---------------
Edit and save me with Ctrl+S!

Architecture:
- Kernel: ANU (Amaryllis is Not Unix) v0.11
- Target: x86_64 hybrid kernel
- Desktop: Double-buffered compositor & floating Capsule Dock
`;

const BOOT_LOG = `[    0.000] ANU Kernel v0.11 (x86_64) booting via bootloader 0.11.17
[    0.012] GDT/TSS initialized (Ring 0 + Ring 3 segments, IST0 Double Fault stack)
[    0.024] PMM Bootstrap & Dynamic tests: [OK] (1024 MiB RAM detected)
[    0.038] VMM 4-level paging + NXE/W^X enabled
[    0.051] ACPI: RSDP/XSDT/FADT/MADT/MCFG parsed — 4 LAPIC cores active (SMP)
[    0.064] PCI scan: VirtIO-Blk, AHCI SATA, Intel 82540EM (e1000), Intel AC'97
[    0.079] Storage: Unified Page Cache (98.2% hit rate) mounted '/' (RAMFS) & '/disk' (FAT32)
[    0.095] IOKit IORegistry initialized; knet_worker & kaudio_mixer spawned
[    0.112] Userland SYSCALL/SYSRET MSRs configured; ELF demos staged
[    0.128] Launching Amaryllis Desktop Compositor...
`;

const AMARYLLIS_TOML = `# Amaryllis Desktop Configuration (/home/amaryllis/.config/amaryllis/desktop.toml)
[theme]
mode = "dark"
surface = "standard"
accent = "mono"

[wallpaper]
preset = "monochrome"

[topbar]
show_clock = true
show_date = true
clock_24h = true
`;

class VirtualFileSystem {
  private root: VfsNode;
  private cwd = '/';
  private listeners = new Set<() => void>();

  constructor() {
    this.root = makeDir('', 'RAMFS');
    this.seed();
  }

  private seed() {
    const home = makeDir('home', 'RAMFS');
    const amaryllis = makeDir('amaryllis', 'RAMFS');
    const config = makeDir('.config', 'RAMFS');
    const cfgAmaryllis = makeDir('amaryllis', 'RAMFS');
    cfgAmaryllis.children!.set('desktop.toml', makeFile('desktop.toml', AMARYLLIS_TOML, 'RAMFS'));
    config.children!.set('amaryllis', cfgAmaryllis);
    amaryllis.children!.set('.config', config);
    home.children!.set('amaryllis', amaryllis);

    const wallpapers = makeDir('wallpapers', 'RAMFS');
    wallpapers.children!.set(
      'nebula_glow.ppm',
      makeFile('nebula_glow.ppm', 'P3\n# Amaryllis procedural nebula glow preview\n64 64\n255\n18 28 52  24 44 80  36 68 118...', 'RAMFS')
    );
    wallpapers.children!.set(
      'slate_grid.bmp',
      makeFile('slate_grid.bmp', 'BM [64x64 24bpp Slate Grid Bitmap]', 'RAMFS')
    );
    home.children!.set('wallpapers', wallpapers);
    home.children!.set('notes.txt', makeFile('notes.txt', NOTES_TXT, 'RAMFS'));

    const tmp = makeDir('tmp', 'RAMFS');

    const disk = makeDir('disk', 'FAT32');
    const audioDir = makeDir('audio', 'FAT32');
    audioDir.children!.set(
      'amaryllis_theme.wav',
      makeFile('amaryllis_theme.wav', 'RIFF....WAVEfmt [44100Hz 16-bit Stereo PCM — Amaryllis Theme]', 'FAT32')
    );
    audioDir.children!.set(
      'startup_chime.wav',
      makeFile('startup_chime.wav', 'RIFF....WAVEfmt [44100Hz 16-bit Stereo PCM — Startup Chime]', 'FAT32')
    );
    disk.children!.set('audio', audioDir);
    disk.children!.set('boot.log', makeFile('boot.log', BOOT_LOG, 'FAT32'));
    disk.children!.set(
      'notes_fat32.txt',
      makeFile('notes_fat32.txt', 'Stored on FAT32 /disk partition (VirtIO-Blk DMA + LFN).\n', 'FAT32')
    );

    this.root.children!.set('home', home);
    this.root.children!.set('tmp', tmp);
    this.root.children!.set('disk', disk);
    this.root.children!.set('README.txt', makeFile('README.txt', README_TXT, 'RAMFS'));

    this.stageElfs();
  }

  stageElfs() {
    const elfs: Array<[string, string, number]> = [
      ['hello.elf', 'Minimal Ring 3 console greeting (SYS_WRITE + SYS_EXIT)', 424],
      ['echo.elf', 'SysV ABI argc/argv demonstration binary', 512],
      ['counter.elf', 'Preemptive Ring 3 worker counter with SYS_SLEEP', 640],
      ['gui_demo.elf', 'SDK Plasma Wave external Ring 3 SHM window', 1280],
      ['net_demo.elf', 'Ring 3 DNS & ICMP Ping syscall client', 896],
      ['lazy_demo.elf', 'Demand-paged SYS_MMAP lazy allocation test', 576],
      ['fault.elf', 'Intentional NULL dereference to test Ring 3 #PF recovery', 384],
      ['guard_fault.elf', 'Stack guard page fault probe', 400],
    ];

    for (const [name, desc, size] of elfs) {
      this.root.children!.set(
        name,
        makeFile(name, `\x7fELF64 [x86_64 ET_EXEC] ${size}B — ${desc}`, 'RAMFS', {
          entry: '0x0000000000400080',
          machine: 'x86_64 (0x3e)',
          type: 'ET_EXEC (0x2)',
          segments: [
            { vaddr: '0x0000000000400000', memsz: size, flags: 'PF_R | PF_X' },
            { vaddr: '0x0000000000600000', memsz: 4096, flags: 'PF_R | PF_W' },
          ],
          description: desc,
        })
      );
    }
    this.emit();
  }

  subscribe(fn: () => void): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  private emit() {
    this.listeners.forEach((fn) => fn());
  }

  getCwd(): string {
    return this.cwd;
  }

  setCwd(path: string): { ok: true; cwd: string } | { ok: false; error: string } {
    const abs = this.resolvePath(path);
    const node = this.getNode(abs);
    if (!node) return { ok: false, error: `No such directory: ${abs}` };
    if (node.kind !== 'directory') return { ok: false, error: `Not a directory: ${abs}` };
    this.cwd = abs;
    this.emit();
    return { ok: true, cwd: abs };
  }

  resolvePath(input: string): string {
    const raw = input.trim();
    const base = raw.startsWith('/') ? [] : this.cwd.split('/').filter(Boolean);
    const parts = raw.split('/').filter(Boolean);
    const stack = [...base];
    for (const part of parts) {
      if (part === '.') continue;
      if (part === '..') {
        stack.pop();
      } else {
        stack.push(part);
      }
    }
    return '/' + stack.join('/');
  }

  private splitParent(absPath: string): [string, string] {
    const clean = this.resolvePath(absPath);
    if (clean === '/') return ['/', ''];
    const idx = clean.lastIndexOf('/');
    const parent = idx === 0 ? '/' : clean.slice(0, idx);
    const leaf = clean.slice(idx + 1);
    return [parent, leaf];
  }

  getNode(absPath: string): VfsNode | null {
    const clean = this.resolvePath(absPath);
    if (clean === '/') return this.root;
    const parts = clean.split('/').filter(Boolean);
    let cur: VfsNode = this.root;
    for (const seg of parts) {
      if (cur.kind !== 'directory' || !cur.children) return null;
      const next = cur.children.get(seg);
      if (!next) return null;
      cur = next;
    }
    return cur;
  }

  listDir(path = this.cwd): VfsDirEntry[] {
    const abs = this.resolvePath(path);
    const node = this.getNode(abs);
    if (!node || node.kind !== 'directory' || !node.children) {
      throw new Error(`Cannot list directory: ${abs}`);
    }
    const entries: VfsDirEntry[] = [];
    for (const child of node.children.values()) {
      const childPath = abs === '/' ? `/${child.name}` : `${abs}/${child.name}`;
      entries.push({
        name: child.name,
        path: childPath,
        kind: child.kind,
        size: child.kind === 'directory' ? child.children?.size ?? 0 : (child.content?.length ?? 0),
        readOnly: Boolean(child.readOnly),
        fsLabel: child.fsLabel ?? (childPath.startsWith('/disk') ? 'FAT32' : 'RAMFS'),
        modifiedAt: child.modifiedAt,
      });
    }
    return entries.sort((a, b) => {
      if (a.kind !== b.kind) return a.kind === 'directory' ? -1 : 1;
      return a.name.localeCompare(b.name);
    });
  }

  readFile(path: string): string {
    const abs = this.resolvePath(path);
    const node = this.getNode(abs);
    if (!node) throw new Error(`File not found: ${abs}`);
    if (node.kind === 'directory') throw new Error(`Is a directory: ${abs}`);
    return node.content ?? '';
  }

  writeFile(path: string, content: string): void {
    const abs = this.resolvePath(path);
    if (abs === '/') throw new Error('Cannot overwrite root directory');
    const [parentPath, leaf] = this.splitParent(abs);
    const parent = this.getNode(parentPath);
    if (!parent || parent.kind !== 'directory' || !parent.children) {
      throw new Error(`Parent directory not found: ${parentPath}`);
    }
    const existing = parent.children.get(leaf);
    if (existing && existing.kind === 'directory') {
      throw new Error(`Target is a directory: ${abs}`);
    }
    const fsLabel = abs.startsWith('/disk') ? 'FAT32' : 'RAMFS';
    parent.children.set(leaf, makeFile(leaf, content, fsLabel, existing?.elfMeta));
    this.emit();
  }

  createDir(path: string): void {
    const abs = this.resolvePath(path);
    if (abs === '/') throw new Error('Root already exists');
    const [parentPath, leaf] = this.splitParent(abs);
    const parent = this.getNode(parentPath);
    if (!parent || parent.kind !== 'directory' || !parent.children) {
      throw new Error(`Parent directory not found: ${parentPath}`);
    }
    if (parent.children.has(leaf)) {
      throw new Error(`Already exists: ${abs}`);
    }
    const fsLabel = abs.startsWith('/disk') ? 'FAT32' : 'RAMFS';
    parent.children.set(leaf, makeDir(leaf, fsLabel));
    this.emit();
  }

  remove(path: string): void {
    const abs = this.resolvePath(path);
    if (abs === '/' || abs === '/disk' || abs === '/home') {
      throw new Error(`Cannot remove protected mount or system dir: ${abs}`);
    }
    const [parentPath, leaf] = this.splitParent(abs);
    const parent = this.getNode(parentPath);
    if (!parent || parent.kind !== 'directory' || !parent.children) {
      throw new Error(`Not found: ${abs}`);
    }
    const target = parent.children.get(leaf);
    if (!target) throw new Error(`Not found: ${abs}`);
    if (target.kind === 'directory' && target.children && target.children.size > 0) {
      throw new Error(`Directory not empty: ${abs}`);
    }
    parent.children.delete(leaf);
    this.emit();
  }

  rename(srcPath: string, newName: string): void {
    const abs = this.resolvePath(srcPath);
    const cleanName = newName.trim().replaceAll('/', '');
    if (!cleanName) throw new Error('Invalid name');
    const [parentPath, oldLeaf] = this.splitParent(abs);
    const parent = this.getNode(parentPath);
    if (!parent || !parent.children) throw new Error('Parent not found');
    const node = parent.children.get(oldLeaf);
    if (!node) throw new Error(`Not found: ${abs}`);
    if (parent.children.has(cleanName)) throw new Error(`Destination exists: ${cleanName}`);
    parent.children.delete(oldLeaf);
    node.name = cleanName;
    node.modifiedAt = Date.now();
    parent.children.set(cleanName, node);
    this.emit();
  }

  copyFile(srcPath: string, destPath: string): void {
    const content = this.readFile(srcPath);
    this.writeFile(destPath, content);
  }

  volumeStats(): VfsVolumeInfo[] {
    const countBytes = (n: VfsNode): number => {
      if (n.kind === 'file') return n.content?.length ?? 0;
      let sum = 0;
      n.children?.forEach((c) => {
        sum += countBytes(c);
      });
      return sum;
    };
    const diskNode = this.getNode('/disk');
    const diskUsed = diskNode ? countBytes(diskNode) : 0;
    const totalUsed = countBytes(this.root);
    return [
      {
        mount: '/',
        fsType: 'RAMFS',
        backend: 'Kernel Heap RAMFS (8 MiB soft cap)',
        readOnly: false,
        capacityBytes: 8 * 1024 * 1024,
        usedBytes: Math.max(0, totalUsed - diskUsed),
        cacheHitRate: '100%',
      },
      {
        mount: '/disk',
        fsType: 'FAT32',
        backend: 'VirtIO-Blk Split-Ring DMA (Page Cache 4KiB)',
        readOnly: false,
        capacityBytes: 64 * 1024 * 1024,
        usedBytes: diskUsed + 4096,
        cacheHitRate: '98.2%',
      },
    ];
  }
}

export const vfs = new VirtualFileSystem();
