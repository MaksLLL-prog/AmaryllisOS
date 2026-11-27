import React, { useEffect, useMemo, useRef, useState } from 'react';
import {
  Activity,
  Bell,
  Download,
  FileText,
  Folder,
  Globe,
  LayoutGrid,
  Monitor,
  Moon,
  Music,
  Power,
  Search,
  Settings,
  SlidersHorizontal,
  Sun,
  Terminal as TerminalIcon,
  Volume2,
  VolumeX,
  Wifi,
} from 'lucide-react';
import { ac97Mixer } from '../os/audio';
import { anuKernel, SystemAlert } from '../os/kernel';
import {
  DEFAULT_DESKTOP_CONFIG,
  derivePalette,
  DesktopConfig,
  ThemePalette,
  WALLPAPER_PRESETS,
} from '../os/theme';
import { vfs } from '../os/vfs';
import { AudioPlayerApp } from './apps/AudioPlayerApp';
import { EditorApp } from './apps/EditorApp';
import { FilesApp } from './apps/FilesApp';
import { MonitorApp } from './apps/MonitorApp';
import { NetViewerApp } from './apps/NetViewerApp';
import { PlasmaApp } from './apps/PlasmaApp';
import { SettingsApp } from './apps/SettingsApp';
import { TerminalApp } from './apps/TerminalApp';
import { FaultModal } from './FaultModal';
import { WallpaperCanvas } from './WallpaperCanvas';

export type BuiltinAppKind =
  | 'files'
  | 'terminal'
  | 'editor'
  | 'audio'
  | 'net'
  | 'settings'
  | 'monitor';

interface DesktopWindow {
  id: number;
  kind: BuiltinAppKind | 'plasma';
  title: string;
  x: number;
  y: number;
  w: number;
  h: number;
  minimized: boolean;
  maximized: boolean;
  restoreRect?: { x: number; y: number; w: number; h: number };
  editorPath?: string;
  wavPath?: string;
  plasmaPid?: number;
  monitorTab?: 'overview' | 'tasks' | 'drivers' | 'alerts';
}

const APP_META: Array<{
  kind: BuiltinAppKind;
  title: string;
  shortLabel: string;
  color: string;
  icon: React.ReactNode;
}> = [
  { kind: 'files', title: 'Files', shortLabel: 'Files', color: '#2884E6', icon: <Folder size={20} /> },
  { kind: 'terminal', title: 'Amaryllis Terminal', shortLabel: 'Terminal', color: '#1F2937', icon: <TerminalIcon size={20} /> },
  { kind: 'editor', title: 'Text Editor', shortLabel: 'Text Editor', color: '#24A87E', icon: <FileText size={20} /> },
  { kind: 'audio', title: 'Audio Player', shortLabel: 'Audio Player', color: '#D99018', icon: <Music size={20} /> },
  { kind: 'net', title: 'Net / HTTP Viewer', shortLabel: 'Net Viewer', color: '#06B6D4', icon: <Globe size={20} /> },
  { kind: 'settings', title: 'System Settings', shortLabel: 'Settings', color: '#64748B', icon: <Settings size={20} /> },
  { kind: 'monitor', title: 'Activity Monitor', shortLabel: 'Activity Monitor', color: '#EF4444', icon: <Activity size={20} /> },
];

interface DesktopProps {
  onExitDesktop: () => void;
}

export const Desktop: React.FC<DesktopProps> = ({ onExitDesktop }) => {
  const [config, setConfig] = useState<DesktopConfig>(DEFAULT_DESKTOP_CONFIG);
  const palette = useMemo(
    () => derivePalette(config.themeMode, config.surfaceStyle, config.accentId),
    [config.themeMode, config.surfaceStyle, config.accentId]
  );

  const [windows, setWindows] = useState<DesktopWindow[]>(() => {
    const sw = typeof window !== 'undefined' ? window.innerWidth : 1280;
    const sh = typeof window !== 'undefined' ? window.innerHeight : 800;
    const termW = Math.min(680, Math.max(420, Math.round(sw * 0.5)));
    const termH = Math.min(460, Math.max(320, sh - 160));
    const filesW = Math.min(540, Math.max(380, Math.round(sw * 0.38)));
    return [
      {
        id: 1,
        kind: 'files',
        title: 'Files',
        x: Math.max(24, sw - filesW - 36),
        y: 56,
        w: filesW,
        h: Math.min(440, sh - 170),
        minimized: false,
        maximized: false,
      },
      {
        id: 2,
        kind: 'terminal',
        title: 'Amaryllis Terminal',
        x: 34,
        y: 66,
        w: termW,
        h: termH,
        minimized: false,
        maximized: false,
      },
    ];
  });

  const nextWinId = useRef(3);
  const [activeMenu, setActiveMenu] = useState<'none' | 'amaryllis' | 'file' | 'edit' | 'view' | 'window'>('none');
  const [spotlightOpen, setSpotlightOpen] = useState(false);
  const [spotlightQuery, setSpotlightQuery] = useState('');
  const [controlCenterOpen, setControlCenterOpen] = useState(false);
  const [calendarOpen, setCalendarOpen] = useState(false);
  const [now, setNow] = useState(() => new Date());
  const [unreadAlerts, setUnreadAlerts] = useState(anuKernel.getUnreadAlertCount());
  const [modalAlert, setModalAlert] = useState<SystemAlert | null>(anuKernel.getActiveModalAlert());
  const [audioMuted, setAudioMuted] = useState(ac97Mixer.getState().muted);
  const [pendingTerminalCmd, setPendingTerminalCmd] = useState<string | null>(null);
  const [snapPreview, setSnapPreview] = useState<'none' | 'left' | 'right' | 'max'>('none');

  const dragRef = useRef<{
    type: 'move' | 'resize';
    winId: number;
    startX: number;
    startY: number;
    origX: number;
    origY: number;
    origW: number;
    origH: number;
  } | null>(null);

  const openPlasmaWindow = (pid: number) => {
    setWindows((prev) => [
      ...prev,
      {
        id: nextWinId.current++,
        kind: 'plasma',
        title: `SDK Plasma Wave (PID ${pid})`,
        x: 160 + (prev.length % 4) * 26,
        y: 96 + (prev.length % 4) * 22,
        w: 410,
        h: 310,
        minimized: false,
        maximized: false,
        plasmaPid: pid,
      },
    ]);
  };

  useEffect(() => {
    anuKernel.setPlasmaLauncher(openPlasmaWindow);
    const unsubKernel = anuKernel.subscribe(() => {
      setUnreadAlerts(anuKernel.getUnreadAlertCount());
      setModalAlert(anuKernel.getActiveModalAlert());
    });
    const unsubAudio = ac97Mixer.subscribe(() => {
      setAudioMuted(ac97Mixer.getState().muted);
    });
    const clockId = window.setInterval(() => setNow(new Date()), 1000);
    return () => {
      unsubKernel();
      unsubAudio();
      window.clearInterval(clockId);
    };
  }, []);

  // Global Alt+Space Spotlight shortcut
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if ((e.altKey && e.code === 'Space') || (e.metaKey && e.key.toLowerCase() === 'k')) {
        e.preventDefault();
        setSpotlightOpen((o) => !o);
        setSpotlightQuery('');
      } else if (e.key === 'Escape') {
        setSpotlightOpen(false);
        setActiveMenu('none');
        setControlCenterOpen(false);
        setCalendarOpen(false);
      }
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, []);

  // Window Drag / Resize handlers
  useEffect(() => {
    const onMouseMove = (e: MouseEvent) => {
      const drag = dragRef.current;
      if (!drag) return;
      const dx = e.clientX - drag.startX;
      const dy = e.clientY - drag.startY;
      const sw = window.innerWidth;
      const sh = window.innerHeight;

      if (drag.type === 'move') {
        const ny = Math.max(28, Math.min(sh - 90, drag.origY + dy));
        const nx = Math.max(-drag.origW + 80, Math.min(sw - 80, drag.origX + dx));
        setWindows((prev) =>
          prev.map((w) => (w.id === drag.winId ? { ...w, x: nx, y: ny, maximized: false } : w))
        );
        if (e.clientX < 24) setSnapPreview('left');
        else if (e.clientX > sw - 24) setSnapPreview('right');
        else if (e.clientY <= 34) setSnapPreview('max');
        else setSnapPreview('none');
      } else {
        const nw = Math.max(380, drag.origW + dx);
        const nh = Math.max(250, drag.origH + dy);
        setWindows((prev) =>
          prev.map((w) => (w.id === drag.winId ? { ...w, w: nw, h: nh, maximized: false } : w))
        );
      }
    };

    const onMouseUp = (e: MouseEvent) => {
      const drag = dragRef.current;
      if (!drag) return;
      const sw = window.innerWidth;
      const sh = window.innerHeight;
      if (drag.type === 'move') {
        if (e.clientX < 24) {
          setWindows((prev) =>
            prev.map((w) =>
              w.id === drag.winId
                ? { ...w, x: 8, y: 34, w: Math.round(sw / 2) - 12, h: sh - 116, maximized: false }
                : w
            )
          );
        } else if (e.clientX > sw - 24) {
          setWindows((prev) =>
            prev.map((w) =>
              w.id === drag.winId
                ? { ...w, x: Math.round(sw / 2) + 4, y: 34, w: Math.round(sw / 2) - 12, h: sh - 116, maximized: false }
                : w
            )
          );
        } else if (e.clientY <= 34) {
          setWindows((prev) =>
            prev.map((w) =>
              w.id === drag.winId
                ? { ...w, x: 8, y: 34, w: sw - 16, h: sh - 116, maximized: true }
                : w
            )
          );
        }
      }
      setSnapPreview('none');
      dragRef.current = null;
    };

    window.addEventListener('mousemove', onMouseMove);
    window.addEventListener('mouseup', onMouseUp);
    return () => {
      window.removeEventListener('mousemove', onMouseMove);
      window.removeEventListener('mouseup', onMouseUp);
    };
  }, []);

  const focusWindow = (id: number) => {
    setWindows((prev) => {
      const target = prev.find((w) => w.id === id);
      if (!target) return prev;
      return [...prev.filter((w) => w.id !== id), { ...target, minimized: false }];
    });
  };

  const openOrFocusApp = (
    kind: BuiltinAppKind,
    opts?: { editorPath?: string; wavPath?: string; monitorTab?: 'overview' | 'tasks' | 'drivers' | 'alerts' }
  ) => {
    setActiveMenu('none');
    setSpotlightOpen(false);

    const existing = windows.find((w) => w.kind === kind);
    if (existing && !opts?.editorPath && !opts?.wavPath && !opts?.monitorTab) {
      focusWindow(existing.id);
      return;
    }

    const meta = APP_META.find((m) => m.kind === kind)!;
    const sw = window.innerWidth;
    const sh = window.innerHeight;
    const defaultW = kind === 'settings' ? 680 : kind === 'files' ? 640 : 660;
    const defaultH = kind === 'audio' ? 450 : 440;
    const offset = (windows.length % 5) * 28;

    setWindows((prev) => {
      const filtered = existing ? prev.filter((w) => w.id !== existing.id) : prev;
      return [
        ...filtered,
        {
          id: existing ? existing.id : nextWinId.current++,
          kind,
          title: meta.title,
          x: Math.min(sw - defaultW - 20, Math.max(24, 90 + offset)),
          y: Math.min(sh - defaultH - 96, Math.max(40, 58 + offset)),
          w: Math.min(defaultW, sw - 32),
          h: Math.min(defaultH, sh - 120),
          minimized: false,
          maximized: false,
          editorPath: opts?.editorPath ?? existing?.editorPath,
          wavPath: opts?.wavPath ?? existing?.wavPath,
          monitorTab: opts?.monitorTab ?? existing?.monitorTab,
        },
      ];
    });
  };

  const closeWindow = (win: DesktopWindow) => {
    if (win.kind === 'plasma' && win.plasmaPid) {
      anuKernel.killProcess(win.plasmaPid);
    }
    setWindows((prev) => prev.filter((w) => w.id !== win.id));
  };

  const toggleMaximize = (id: number) => {
    const sw = window.innerWidth;
    const sh = window.innerHeight;
    setWindows((prev) =>
      prev.map((w) => {
        if (w.id !== id) return w;
        if (w.maximized && w.restoreRect) {
          return { ...w, ...w.restoreRect, maximized: false, restoreRect: undefined };
        }
        return {
          ...w,
          restoreRect: { x: w.x, y: w.y, w: w.w, h: w.h },
          x: 8,
          y: 34,
          w: sw - 16,
          h: sh - 116,
          maximized: true,
        };
      })
    );
  };

  const handleExecElfFromDesktop = (path: string) => {
    if (path.includes('fault')) {
      anuKernel.triggerFaultElf(path);
      return;
    }
    if (path.includes('gui_demo')) {
      anuKernel.spawnBackgroundElf(path);
      return;
    }
    openOrFocusApp('terminal');
    setPendingTerminalCmd(`exec ${path}`);
  };

  const activeWindow = [...windows].reverse().find((w) => !w.minimized) ?? null;
  const activeAppLabel = activeWindow
    ? APP_META.find((m) => m.kind === activeWindow.kind)?.shortLabel ?? 'SDK App'
    : 'Desktop';

  const clockStr = (() => {
    const parts: string[] = [];
    if (config.showDate) {
      parts.push(now.toLocaleDateString('en-US', { weekday: 'short', month: 'short', day: '2-digit' }));
    }
    if (config.showClock) {
      parts.push(
        now.toLocaleTimeString('en-US', {
          hour: '2-digit',
          minute: '2-digit',
          hour12: !config.clock24h,
        })
      );
    }
    return parts.join('  ');
  })();

  const spotlightItems = useMemo(() => {
    const q = spotlightQuery.trim().toLowerCase();
    const list: Array<{ label: string; subtitle: string; run: () => void }> = [
      ...APP_META.map((m) => ({
        label: m.title,
        subtitle: `Open built-in ${m.shortLabel} application`,
        run: () => openOrFocusApp(m.kind),
      })),
      {
        label: 'Switch Black / White Theme',
        subtitle: `Current: ${config.themeMode === 'dark' ? 'Black (Dark)' : 'White (Light)'}`,
        run: () => {
          setConfig((c) => ({ ...c, themeMode: c.themeMode === 'dark' ? 'light' : 'dark' }));
          setSpotlightOpen(false);
        },
      },
      {
        label: 'Next Procedural Wallpaper',
        subtitle: 'Cycle silk-wave desktop background',
        run: () => {
          const idx = WALLPAPER_PRESETS.findIndex((p) => p.id === config.wallpaper);
          const next = WALLPAPER_PRESETS[(idx + 1) % WALLPAPER_PRESETS.length];
          setConfig((c) => ({ ...c, wallpaper: next.id }));
          setSpotlightOpen(false);
        },
      },
      {
        label: 'Play Audio Chime',
        subtitle: "Test non-blocking Intel AC'97 audio mixer",
        run: () => {
          ac97Mixer.beep(587, 110);
          setSpotlightOpen(false);
        },
      },
      {
        label: 'Run /gui_demo.elf (Ring 3 Plasma)',
        subtitle: 'Launch amaryllis-sdk SHM Plasma Wave window',
        run: () => {
          setSpotlightOpen(false);
          handleExecElfFromDesktop('/gui_demo.elf');
        },
      },
      {
        label: 'Run /fault.elf (Test Ring 3 #PF Isolation)',
        subtitle: 'Trigger Ring 3 Page Fault & open Fault Inspector',
        run: () => {
          setSpotlightOpen(false);
          handleExecElfFromDesktop('/fault.elf');
        },
      },
      {
        label: 'Exit to Text Console',
        subtitle: 'Drop cleanly into raw VGA/FB amsh shell',
        run: () => {
          setSpotlightOpen(false);
          onExitDesktop();
        },
      },
    ];

    try {
      for (const e of vfs.listDir('/')) {
        if (e.name.endsWith('.elf')) {
          list.push({
            label: `Run ${e.path}`,
            subtitle: `Ring 3 ELF64 binary (${e.size} B)`,
            run: () => {
              setSpotlightOpen(false);
              handleExecElfFromDesktop(e.path);
            },
          });
        }
      }
    } catch {
      // ignore
    }

    if (!q) return list;
    return list.filter(
      (i) => i.label.toLowerCase().includes(q) || i.subtitle.toLowerCase().includes(q)
    );
  }, [spotlightQuery, config.themeMode, config.wallpaper]);

  return (
    <div
      className="fixed inset-0 overflow-hidden select-none"
      onClick={() => {
        setActiveMenu('none');
        setControlCenterOpen(false);
        setCalendarOpen(false);
      }}
    >
      <WallpaperCanvas preset={config.wallpaper} themeMode={config.themeMode} />

      {/* 28px Translucent Top Bar */}
      <header
        className="fixed top-0 left-0 right-0 h-[28px] z-50 flex items-center justify-between px-2.5 border-b backdrop-blur-md text-[12px]"
        style={{
          backgroundColor: palette.topbarBg,
          borderColor: palette.topbarBorder,
          color: palette.topbarText,
        }}
        onClick={(e) => e.stopPropagation()}
      >
        {/* Left Menus */}
        <div className="flex items-center gap-0.5 relative">
          {(
            [
              ['amaryllis', activeAppLabel, true],
              ['file', 'File', false],
              ['edit', 'Edit', false],
              ['view', 'View', false],
              ['window', 'Window', false],
            ] as const
          ).map(([key, label, bold]) => (
            <button
              key={key}
              onClick={() => setActiveMenu((m) => (m === key ? 'none' : key))}
              className={`px-2.5 h-[22px] rounded flex items-center cursor-pointer ${bold ? 'font-semibold' : ''}`}
              style={{
                backgroundColor: activeMenu === key ? palette.surfaceSelected : 'transparent',
              }}
            >
              {label}
            </button>
          ))}

          {/* Top Bar Dropdown */}
          {activeMenu !== 'none' && (
            <div
              className="absolute top-[28px] left-0 min-w-[220px] rounded-lg border shadow-2xl py-1.5 z-[100]"
              style={{
                backgroundColor: palette.modalBg,
                borderColor: palette.modalBorder,
                color: palette.textPrimary,
                left:
                  activeMenu === 'amaryllis'
                    ? 0
                    : activeMenu === 'file'
                      ? 84
                      : activeMenu === 'edit'
                        ? 124
                        : activeMenu === 'view'
                          ? 166
                          : 212,
              }}
            >
              {activeMenu === 'amaryllis' && (
                <>
                  <MenuItem label="About Amaryllis OS v0.11" onClick={() => { openOrFocusApp('terminal'); setPendingTerminalCmd('neofetch'); }} palette={palette} />
                  <MenuItem label="System Settings..." onClick={() => openOrFocusApp('settings')} palette={palette} />
                  <MenuItem label="Activity Monitor" onClick={() => openOrFocusApp('monitor')} palette={palette} />
                  <div className="my-1 border-t" style={{ borderColor: palette.surfaceBorder }} />
                  <MenuItem label="Spotlight Search (Alt+Space)" onClick={() => { setActiveMenu('none'); setSpotlightOpen(true); }} palette={palette} />
                  <MenuItem label="Exit to Text Console" onClick={onExitDesktop} palette={palette} danger />
                </>
              )}
              {activeMenu === 'file' && (
                <>
                  <MenuItem label="New Terminal Window" onClick={() => openOrFocusApp('terminal')} palette={palette} />
                  <MenuItem label="Open Files Explorer" onClick={() => openOrFocusApp('files')} palette={palette} />
                  <MenuItem label="Open Text Editor" onClick={() => openOrFocusApp('editor')} palette={palette} />
                  <MenuItem label="Stage Ring 3 ELFs (mkelf)" onClick={() => { vfs.stageElfs(); setActiveMenu('none'); }} palette={palette} />
                </>
              )}
              {activeMenu === 'edit' && (
                <>
                  <MenuItem label="Open /home/notes.txt" onClick={() => openOrFocusApp('editor', { editorPath: '/home/notes.txt' })} palette={palette} />
                  <MenuItem label="Open /README.txt" onClick={() => openOrFocusApp('editor', { editorPath: '/README.txt' })} palette={palette} />
                  <MenuItem label="Open /disk/boot.log" onClick={() => openOrFocusApp('editor', { editorPath: '/disk/boot.log' })} palette={palette} />
                </>
              )}
              {activeMenu === 'view' && (
                <>
                  <MenuItem
                    label={`Switch to ${config.themeMode === 'dark' ? 'White (Light)' : 'Black (Dark)'} Theme`}
                    onClick={() => {
                      setConfig((c) => ({ ...c, themeMode: c.themeMode === 'dark' ? 'light' : 'dark' }));
                      setActiveMenu('none');
                    }}
                    palette={palette}
                  />
                  <MenuItem
                    label="Next Wallpaper Preset"
                    onClick={() => {
                      const idx = WALLPAPER_PRESETS.findIndex((p) => p.id === config.wallpaper);
                      const next = WALLPAPER_PRESETS[(idx + 1) % WALLPAPER_PRESETS.length];
                      setConfig((c) => ({ ...c, wallpaper: next.id }));
                      setActiveMenu('none');
                    }}
                    palette={palette}
                  />
                </>
              )}
              {activeMenu === 'window' && (
                <>
                  {windows.length === 0 ? (
                    <div className="px-3 py-1.5 text-[11px]" style={{ color: palette.textMuted }}>
                      No open windows
                    </div>
                  ) : (
                    windows.map((w) => (
                      <MenuItem
                        key={w.id}
                        label={`${w.minimized ? '• ' : ''}${w.title}`}
                        onClick={() => {
                          focusWindow(w.id);
                          setActiveMenu('none');
                        }}
                        palette={palette}
                      />
                    ))
                  )}
                </>
              )}
            </div>
          )}
        </div>

        {/* Right Tray */}
        <div className="flex items-center gap-1.5">
          <a
            href="/api/download/amaryllis-os.zip"
            download="AmaryllisOS-cow-mach.zip"
            className="px-2 h-[22px] rounded flex items-center gap-1 hover:bg-white/10 cursor-pointer text-[11px] font-medium"
            title="Download AmaryllisOS Rust source (.zip)"
          >
            <Download size={12} />
            <span>SRC.zip</span>
          </a>
          <button
            onClick={() => openOrFocusApp('monitor', { monitorTab: 'alerts' })}
            className="px-1.5 h-[22px] rounded flex items-center gap-1 hover:bg-white/10 cursor-pointer"
            title="System Alerts & Fault Inspector"
          >
            <Bell size={13} className={unreadAlerts > 0 ? 'text-[#DE4B5A]' : ''} />
            {unreadAlerts > 0 && (
              <span className="text-[10px] font-bold text-[#DE4B5A]">{unreadAlerts}</span>
            )}
          </button>
          <button
            onClick={() => setConfig((c) => ({ ...c, themeMode: c.themeMode === 'dark' ? 'light' : 'dark' }))}
            className="px-1.5 h-[22px] rounded flex items-center hover:bg-white/10 cursor-pointer"
            title="Toggle Black / White Theme"
          >
            {config.themeMode === 'dark' ? <Moon size={13} /> : <Sun size={13} />}
          </button>
          <button
            onClick={() => openOrFocusApp('net')}
            className="px-1.5 h-[22px] rounded flex items-center hover:bg-white/10 cursor-pointer"
            title="Intel e1000 Network"
          >
            <Wifi size={13} style={{ color: palette.accentMint }} />
          </button>
          <button
            onClick={() => ac97Mixer.toggleMute()}
            className="px-1.5 h-[22px] rounded flex items-center hover:bg-white/10 cursor-pointer"
            title="Toggle AC'97 Audio Mute"
          >
            {audioMuted ? <VolumeX size={13} /> : <Volume2 size={13} />}
          </button>
          <button
            onClick={() => {
              setSpotlightOpen((o) => !o);
              setSpotlightQuery('');
            }}
            className="px-1.5 h-[22px] rounded flex items-center hover:bg-white/10 cursor-pointer"
            title="Spotlight Search (Alt+Space)"
          >
            <Search size={13} />
          </button>
          <button
            onClick={() => {
              setControlCenterOpen((o) => !o);
              setCalendarOpen(false);
            }}
            className="px-1.5 h-[22px] rounded flex items-center hover:bg-white/10 cursor-pointer"
            title="Control Center"
          >
            <SlidersHorizontal size={13} />
          </button>
          {clockStr && (
            <button
              onClick={() => {
                setCalendarOpen((o) => !o);
                setControlCenterOpen(false);
              }}
              className="px-2 h-[22px] rounded hover:bg-white/10 cursor-pointer font-medium"
            >
              {clockStr}
            </button>
          )}
        </div>
      </header>

      {/* Snap Preview Overlay */}
      {snapPreview !== 'none' && (
        <div
          className="fixed z-30 rounded-xl border-2 pointer-events-none transition-all duration-100"
          style={{
            backgroundColor: 'rgba(56, 144, 240, 0.22)',
            borderColor: '#60AAFA',
            top: 34,
            left: snapPreview === 'right' ? '50%' : 8,
            width: snapPreview === 'max' ? 'calc(100vw - 16px)' : 'calc(50vw - 12px)',
            height: 'calc(100vh - 116px)',
          }}
        />
      )}

      {/* Windows */}
      {windows.map((win, idx) => {
        if (win.minimized) return null;
        const isActive = idx === windows.length - 1;
        return (
          <div
            key={win.id}
            onMouseDown={() => focusWindow(win.id)}
            style={{
              left: win.x,
              top: win.y,
              width: win.w,
              height: win.h,
              zIndex: 10 + idx,
              backgroundColor: isActive ? palette.winBgActive : palette.winBgInactive,
              borderColor: isActive ? palette.winBorderActive : palette.winBorderInactive,
            }}
            className="fixed rounded-[11px] border shadow-[0_20px_50px_rgba(0,0,0,0.45)] flex flex-col overflow-hidden"
          >
            {/* Titlebar */}
            <div
              onMouseDown={(e) => {
                if ((e.target as HTMLElement).closest('button')) return;
                focusWindow(win.id);
                dragRef.current = {
                  type: 'move',
                  winId: win.id,
                  startX: e.clientX,
                  startY: e.clientY,
                  origX: win.x,
                  origY: win.y,
                  origW: win.w,
                  origH: win.h,
                };
              }}
              onDoubleClick={() => toggleMaximize(win.id)}
              className="h-[32px] shrink-0 px-3 flex items-center justify-between border-b cursor-grab active:cursor-grabbing"
              style={{
                backgroundColor: isActive ? palette.winTitleActive : palette.winTitleInactive,
                borderColor: palette.winDivider,
              }}
            >
              <div className="flex items-center gap-2">
                <button
                  onClick={() => closeWindow(win)}
                  className="w-3 h-3 rounded-full bg-[#FF5F57] hover:brightness-110 cursor-pointer"
                  title="Close"
                />
                <button
                  onClick={() =>
                    setWindows((prev) =>
                      prev.map((w) => (w.id === win.id ? { ...w, minimized: true } : w))
                    )
                  }
                  className="w-3 h-3 rounded-full bg-[#FEBC2E] hover:brightness-110 cursor-pointer"
                  title="Minimize"
                />
                <button
                  onClick={() => toggleMaximize(win.id)}
                  className="w-3 h-3 rounded-full bg-[#28C840] hover:brightness-110 cursor-pointer"
                  title="Maximize"
                />
              </div>
              <div
                className="text-[12px] font-semibold truncate px-3"
                style={{ color: isActive ? palette.textPrimary : palette.textSecondary }}
              >
                {win.title}
              </div>
              <div className="w-12" />
            </div>

            {/* App Body */}
            <div className="flex-1 min-h-0 overflow-hidden relative">
              {win.kind === 'terminal' && (
                <TerminalApp
                  palette={palette}
                  pendingCommand={pendingTerminalCmd}
                  onConsumePendingCommand={() => setPendingTerminalCmd(null)}
                  onExitDesktop={onExitDesktop}
                  onOpenPlasma={openPlasmaWindow}
                />
              )}
              {win.kind === 'files' && (
                <FilesApp
                  palette={palette}
                  onOpenEditor={(path) => openOrFocusApp('editor', { editorPath: path })}
                  onExecElf={handleExecElfFromDesktop}
                  onPlayAudio={(path) => openOrFocusApp('audio', { wavPath: path })}
                />
              )}
              {win.kind === 'editor' && (
                <EditorApp palette={palette} initialPath={win.editorPath} />
              )}
              {win.kind === 'audio' && (
                <AudioPlayerApp palette={palette} initialWavPath={win.wavPath} />
              )}
              {win.kind === 'net' && <NetViewerApp palette={palette} />}
              {win.kind === 'settings' && (
                <SettingsApp
                  palette={palette}
                  config={config}
                  onUpdateConfig={(patch) => setConfig((c) => ({ ...c, ...patch }))}
                />
              )}
              {win.kind === 'monitor' && (
                <MonitorApp palette={palette} initialTab={win.monitorTab} />
              )}
              {win.kind === 'plasma' && (
                <PlasmaApp
                  pid={win.plasmaPid ?? 100}
                  palette={palette}
                  onClose={() => closeWindow(win)}
                />
              )}

              {/* Resize Handle */}
              <div
                onMouseDown={(e) => {
                  e.stopPropagation();
                  focusWindow(win.id);
                  dragRef.current = {
                    type: 'resize',
                    winId: win.id,
                    startX: e.clientX,
                    startY: e.clientY,
                    origX: win.x,
                    origY: win.y,
                    origW: win.w,
                    origH: win.h,
                  };
                }}
                className="absolute bottom-0 right-0 w-3.5 h-3.5 cursor-nwse-resize"
              />
            </div>
          </div>
        );
      })}

      {/* Floating Glass Dock Capsule */}
      <div className="fixed bottom-3 left-1/2 -translate-x-1/2 z-50" onClick={(e) => e.stopPropagation()}>
        <div
          className="h-[60px] px-3 rounded-[18px] border backdrop-blur-xl shadow-[0_14px_40px_rgba(0,0,0,0.45)] flex items-center gap-2"
          style={{
            backgroundColor: palette.dockBg,
            borderColor: palette.dockBorder,
            color: palette.topbarText,
          }}
        >
          {/* Launcher */}
          <button
            onClick={() => {
              setSpotlightOpen((o) => !o);
              setSpotlightQuery('');
            }}
            className="w-[42px] h-[42px] rounded-xl flex items-center justify-center hover:scale-105 transition-transform cursor-pointer"
            style={{ backgroundColor: palette.surfaceCard }}
            title="App Launcher (Alt+Space)"
          >
            <LayoutGrid size={20} style={{ color: palette.accentMint }} />
          </button>

          <div className="h-7 w-[1px] mx-0.5" style={{ backgroundColor: palette.dockDivider }} />

          {/* 7 Built-in Apps */}
          {APP_META.map((app) => {
            const isOpen = windows.some((w) => w.kind === app.kind);
            return (
              <button
                key={app.kind}
                onClick={() => openOrFocusApp(app.kind)}
                className="relative w-[42px] h-[42px] rounded-xl flex items-center justify-center text-white hover:-translate-y-1 transition-transform cursor-pointer shadow-sm group"
                style={{ backgroundColor: app.color }}
                title={app.title}
              >
                {app.icon}
                {isOpen && (
                  <span
                    className="absolute -bottom-1.5 w-1.5 h-1.5 rounded-full"
                    style={{ backgroundColor: palette.dockDot }}
                  />
                )}
              </button>
            );
          })}

          <div className="h-7 w-[1px] mx-0.5" style={{ backgroundColor: palette.dockDivider }} />

          {/* Status / Control Center */}
          <button
            onClick={() => {
              setControlCenterOpen((o) => !o);
              setCalendarOpen(false);
            }}
            className="w-[38px] h-[38px] rounded-xl flex items-center justify-center hover:bg-white/10 cursor-pointer"
            title="Control Center"
          >
            <Monitor size={18} />
          </button>

          {/* Power / Exit to Text Console */}
          <button
            onClick={onExitDesktop}
            className="w-[38px] h-[38px] rounded-xl flex items-center justify-center hover:bg-[#DE4B5A]/20 text-[#DE4B5A] cursor-pointer"
            title="Exit to Text Console"
          >
            <Power size={18} />
          </button>
        </div>
      </div>

      {/* Control Center Popover */}
      {controlCenterOpen && (
        <div
          onClick={(e) => e.stopPropagation()}
          className="fixed bottom-[82px] right-6 w-[264px] rounded-xl border p-3.5 shadow-2xl z-[90] space-y-3 text-[12px]"
          style={{
            backgroundColor: palette.modalBg,
            borderColor: palette.modalBorder,
            color: palette.textPrimary,
          }}
        >
          <div className="font-semibold flex justify-between items-center">
            <span>Control Center</span>
            <span className="text-[10px]" style={{ color: palette.accentMint }}>ANU v0.11</span>
          </div>
          <div className="grid grid-cols-2 gap-2">
            <button
              onClick={() => setConfig((c) => ({ ...c, themeMode: c.themeMode === 'dark' ? 'light' : 'dark' }))}
              className="p-2.5 rounded-lg border text-left cursor-pointer"
              style={{ backgroundColor: palette.surfaceCard, borderColor: palette.surfaceBorder }}
            >
              <div className="font-medium">{config.themeMode === 'dark' ? 'Dark Mode' : 'Light Mode'}</div>
              <div className="text-[10px]" style={{ color: palette.textMuted }}>Click to switch</div>
            </button>
            <button
              onClick={() => {
                const idx = WALLPAPER_PRESETS.findIndex((p) => p.id === config.wallpaper);
                const next = WALLPAPER_PRESETS[(idx + 1) % WALLPAPER_PRESETS.length];
                setConfig((c) => ({ ...c, wallpaper: next.id }));
              }}
              className="p-2.5 rounded-lg border text-left cursor-pointer"
              style={{ backgroundColor: palette.surfaceCard, borderColor: palette.surfaceBorder }}
            >
              <div className="font-medium">Wallpaper</div>
              <div className="text-[10px] truncate" style={{ color: palette.textMuted }}>
                {WALLPAPER_PRESETS.find((p) => p.id === config.wallpaper)?.shortTitle}
              </div>
            </button>
          </div>
          <div className="p-2.5 rounded-lg border space-y-1.5" style={{ backgroundColor: palette.surfaceCard, borderColor: palette.surfaceBorder }}>
            <div className="flex justify-between text-[11px]">
              <span>AC&apos;97 Audio</span>
              <span>{ac97Mixer.getState().volume}%</span>
            </div>
            <input
              type="range"
              min={0}
              max={100}
              defaultValue={ac97Mixer.getState().volume}
              onChange={(e) => ac97Mixer.setVolume(Number(e.target.value))}
              className="w-full accent-[#24A87E]"
            />
          </div>
        </div>
      )}

      {/* Calendar Popover */}
      {calendarOpen && (
        <div
          onClick={(e) => e.stopPropagation()}
          className="fixed top-[34px] right-3 w-[248px] rounded-xl border p-3.5 shadow-2xl z-[90] text-[12px]"
          style={{
            backgroundColor: palette.modalBg,
            borderColor: palette.modalBorder,
            color: palette.textPrimary,
          }}
        >
          <div className="font-semibold mb-1">
            {now.toLocaleDateString('en-US', { month: 'long', year: 'numeric' })}
          </div>
          <div className="text-[11px] mb-2" style={{ color: palette.textSecondary }}>
            CMOS RTC: {now.toLocaleDateString('en-US', { weekday: 'long', day: 'numeric' })}
          </div>
          <div className="grid grid-cols-7 gap-1 text-center text-[10px]" style={{ color: palette.textMuted }}>
            {['Su', 'Mo', 'Tu', 'We', 'Th', 'Fr', 'Sa'].map((d) => (
              <div key={d} className="font-semibold">{d}</div>
            ))}
            {Array.from({ length: 28 }, (_, i) => i + 1).map((day) => (
              <div
                key={day}
                className="py-1 rounded"
                style={{
                  backgroundColor: day === now.getDate() ? palette.accentPrimary : 'transparent',
                  color: day === now.getDate() ? palette.onAccent : palette.textPrimary,
                }}
              >
                {day}
              </div>
            ))}
          </div>
        </div>
      )}

      {/* Spotlight Launcher */}
      {spotlightOpen && (
        <div
          className="fixed inset-0 z-[200] flex items-start justify-center pt-24 bg-black/35"
          onClick={() => setSpotlightOpen(false)}
        >
          <div
            onClick={(e) => e.stopPropagation()}
            className="w-full max-w-[520px] rounded-xl border shadow-2xl overflow-hidden"
            style={{
              backgroundColor: palette.modalBg,
              borderColor: palette.modalBorder,
              color: palette.textPrimary,
            }}
          >
            <div className="flex items-center gap-2.5 px-3.5 py-2.5 border-b" style={{ borderColor: palette.surfaceBorder }}>
              <Search size={16} style={{ color: palette.textMuted }} />
              <input
                autoFocus
                value={spotlightQuery}
                onChange={(e) => setSpotlightQuery(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter' && spotlightItems[0]) {
                    spotlightItems[0].run();
                  }
                }}
                placeholder="Search apps, Ring 3 ELFs, or system actions..."
                className="flex-1 bg-transparent outline-none text-[13px]"
                style={{ color: palette.textPrimary }}
              />
            </div>
            <div className="max-h-[320px] overflow-y-auto p-1.5">
              {spotlightItems.map((item, idx) => (
                <button
                  key={idx}
                  onClick={item.run}
                  className="w-full px-3 py-2 rounded-lg text-left flex flex-col hover:bg-white/10 cursor-pointer"
                >
                  <span className="text-[12.5px] font-medium">{item.label}</span>
                  <span className="text-[11px]" style={{ color: palette.textSecondary }}>
                    {item.subtitle}
                  </span>
                </button>
              ))}
            </div>
          </div>
        </div>
      )}

      {/* Fault Inspector Modal */}
      {modalAlert && (
        <FaultModal
          alert={modalAlert}
          palette={palette}
          onDismiss={() => setModalAlert(null)}
        />
      )}
    </div>
  );
};

interface MenuItemProps {
  label: string;
  onClick: () => void;
  palette: ThemePalette;
  danger?: boolean;
}

const MenuItem: React.FC<MenuItemProps> = ({ label, onClick, palette, danger }) => (
  <button
    onClick={onClick}
    className="w-full px-3 py-1.5 text-left text-[12px] hover:bg-white/10 cursor-pointer flex items-center justify-between"
    style={{ color: danger ? palette.accentErr : palette.textPrimary }}
  >
    <span>{label}</span>
  </button>
);
