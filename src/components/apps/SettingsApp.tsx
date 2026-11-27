import React, { useState } from 'react';
import { FileText, Layers, LayoutGrid, Monitor, Sun, Volume2, Wifi } from 'lucide-react';
import { ac97Mixer } from '../../os/audio';
import { anuKernel } from '../../os/kernel';
import {
  ACCENT_PRESETS,
  DesktopConfig,
  SurfaceStyle,
  ThemePalette,
  WALLPAPER_PRESETS,
} from '../../os/theme';

type SectionId =
  | 'appearance'
  | 'wallpaper'
  | 'desktop'
  | 'windows'
  | 'typography'
  | 'sound'
  | 'network';

interface SettingsAppProps {
  palette: ThemePalette;
  config: DesktopConfig;
  onUpdateConfig: (patch: Partial<DesktopConfig>) => void;
}

const SECTIONS: Array<{ id: SectionId; label: string; icon: React.ReactNode }> = [
  { id: 'appearance', label: 'Appearance', icon: <Sun size={14} /> },
  { id: 'wallpaper', label: 'Wallpaper', icon: <LayoutGrid size={14} /> },
  { id: 'desktop', label: 'Desktop & Bar', icon: <Monitor size={14} /> },
  { id: 'windows', label: 'Windows & Motion', icon: <Layers size={14} /> },
  { id: 'typography', label: 'Typography', icon: <FileText size={14} /> },
  { id: 'sound', label: 'Sound', icon: <Volume2 size={14} /> },
  { id: 'network', label: 'Network', icon: <Wifi size={14} /> },
];

export const SettingsApp: React.FC<SettingsAppProps> = ({ palette, config, onUpdateConfig }) => {
  const [section, setSection] = useState<SectionId>('appearance');
  const [audioState, setAudioState] = useState(ac97Mixer.getState());
  const net = anuKernel.getNetState();

  return (
    <div
      className="flex h-full w-full text-[12px]"
      style={{ backgroundColor: palette.winBgActive, color: palette.textPrimary }}
    >
      {/* Sidebar */}
      <div
        className="w-44 shrink-0 border-r p-2 flex flex-col gap-1"
        style={{ backgroundColor: palette.surfaceSidebar, borderColor: palette.surfaceBorder }}
      >
        {SECTIONS.map((s) => (
          <button
            key={s.id}
            onClick={() => setSection(s.id)}
            className="flex items-center gap-2.5 px-2.5 py-2 rounded text-left cursor-pointer transition-colors"
            style={{
              backgroundColor: section === s.id ? palette.surfaceSelected : 'transparent',
              color: section === s.id ? palette.textPrimary : palette.textSecondary,
            }}
          >
            {s.icon}
            <span className="font-medium">{s.label}</span>
          </button>
        ))}
      </div>

      {/* Content */}
      <div className="flex-1 overflow-y-auto p-5 space-y-5">
        {section === 'appearance' && (
          <>
            <div>
              <h3 className="text-[14px] font-semibold mb-1">Theme Mode</h3>
              <p className="text-[11px] mb-3" style={{ color: palette.textSecondary }}>
                Switch between Amaryllis Dark (Black) and Light (White) desktop palettes.
              </p>
              <div className="flex gap-2">
                {(['dark', 'light'] as const).map((m) => (
                  <button
                    key={m}
                    onClick={() => onUpdateConfig({ themeMode: m })}
                    className="px-4 py-2 rounded-lg border font-medium cursor-pointer capitalize"
                    style={{
                      backgroundColor: config.themeMode === m ? palette.accentPrimary : palette.surfaceCard,
                      color: config.themeMode === m ? palette.onAccent : palette.textPrimary,
                      borderColor: palette.surfaceBorder,
                    }}
                  >
                    {m === 'dark' ? 'Black (Dark)' : 'White (Light)'}
                  </button>
                ))}
              </div>
            </div>

            <div>
              <h3 className="text-[14px] font-semibold mb-2">Surface Style</h3>
              <div className="flex gap-2">
                {(
                  [
                    ['standard', 'Default'],
                    ['midnight', 'OLED Black'],
                    ['snow', 'Pure White'],
                  ] as Array<[SurfaceStyle, string]>
                ).map(([id, label]) => (
                  <button
                    key={id}
                    onClick={() => onUpdateConfig({ surfaceStyle: id })}
                    className="px-3 py-1.5 rounded border cursor-pointer"
                    style={{
                      backgroundColor: config.surfaceStyle === id ? palette.surfaceSelected : palette.surfaceCard,
                      borderColor: config.surfaceStyle === id ? palette.accentPrimary : palette.surfaceBorder,
                    }}
                  >
                    {label}
                  </button>
                ))}
              </div>
            </div>

            <div>
              <h3 className="text-[14px] font-semibold mb-2">Accent Color</h3>
              <div className="grid grid-cols-4 gap-2">
                {ACCENT_PRESETS.map((acc) => {
                  const active = config.accentId === acc.id;
                  return (
                    <button
                      key={acc.id}
                      onClick={() => onUpdateConfig({ accentId: acc.id })}
                      className="flex items-center gap-2 px-2.5 py-2 rounded border cursor-pointer"
                      style={{
                        backgroundColor: active ? palette.surfaceSelected : palette.surfaceCard,
                        borderColor: active ? palette.accentPrimary : palette.surfaceBorder,
                      }}
                    >
                      <span
                        className="w-3.5 h-3.5 rounded-full border border-white/20"
                        style={{ backgroundColor: acc.color }}
                      />
                      <span>{acc.label}</span>
                    </button>
                  );
                })}
              </div>
            </div>
          </>
        )}

        {section === 'wallpaper' && (
          <div>
            <h3 className="text-[14px] font-semibold mb-1">Procedural Silk-Wave Wallpapers</h3>
            <p className="text-[11px] mb-3" style={{ color: palette.textSecondary }}>
              4-layer procedural curves rendered in real time by the Amaryllis Compositor.
            </p>
            <div className="grid grid-cols-2 gap-3">
              {WALLPAPER_PRESETS.map((wp) => {
                const active = config.wallpaper === wp.id;
                return (
                  <button
                    key={wp.id}
                    onClick={() => onUpdateConfig({ wallpaper: wp.id })}
                    className="p-2.5 rounded-lg border text-left cursor-pointer flex flex-col gap-2"
                    style={{
                      backgroundColor: active ? palette.surfaceSelected : palette.surfaceCard,
                      borderColor: active ? palette.accentPrimary : palette.surfaceBorder,
                    }}
                  >
                    <div
                      className="h-14 w-full rounded"
                      style={{
                        background: `linear-gradient(135deg, ${wp.preview[0]}, ${wp.preview[1]}, ${wp.preview[2]})`,
                      }}
                    />
                    <div className="font-medium">{wp.title}</div>
                  </button>
                );
              })}
            </div>
          </div>
        )}

        {section === 'desktop' && (
          <div className="space-y-3">
            <h3 className="text-[14px] font-semibold">Top Bar &amp; Capsule Dock</h3>
            {(
              [
                ['showClock', 'Show RTC Clock in Top Bar'],
                ['showDate', 'Show Date in Top Bar'],
                ['clock24h', 'Use 24-Hour Clock Format'],
                ['dockAutoHide', 'Auto-Hide Floating Dock'],
              ] as Array<[keyof DesktopConfig, string]>
            ).map(([key, label]) => (
              <label
                key={key}
                className="flex items-center justify-between p-3 rounded border cursor-pointer"
                style={{ backgroundColor: palette.surfaceCard, borderColor: palette.surfaceBorder }}
              >
                <span>{label}</span>
                <input
                  type="checkbox"
                  checked={Boolean(config[key])}
                  onChange={(e) => onUpdateConfig({ [key]: e.target.checked })}
                  className="w-4 h-4 accent-[#24A87E]"
                />
              </label>
            ))}
          </div>
        )}

        {section === 'windows' && (
          <div className="space-y-3">
            <h3 className="text-[14px] font-semibold">Window Manager &amp; Compositor</h3>
            <label
              className="flex items-center justify-between p-3 rounded border cursor-pointer"
              style={{ backgroundColor: palette.surfaceCard, borderColor: palette.surfaceBorder }}
            >
              <span>Enable Smooth Open/Close Window Motion</span>
              <input
                type="checkbox"
                checked={config.animations}
                onChange={(e) => onUpdateConfig({ animations: e.target.checked })}
                className="w-4 h-4 accent-[#24A87E]"
              />
            </label>
            <div className="p-3 rounded border text-[11px]" style={{ backgroundColor: palette.surfaceCard, borderColor: palette.surfaceBorder, color: palette.textSecondary }}>
              Drag any window titlebar to the left/right screen edge or top edge to trigger Left Half, Right Half, or Maximize snapping.
            </div>
          </div>
        )}

        {section === 'typography' && (
          <div className="space-y-3">
            <h3 className="text-[14px] font-semibold">Monospace Terminal &amp; Editor Font</h3>
            <div className="flex gap-2">
              {(['JetBrainsMono', 'AmaryllisMono'] as const).map((font) => (
                <button
                  key={font}
                  onClick={() => onUpdateConfig({ monoFont: font })}
                  className="px-4 py-2 rounded border cursor-pointer amaryllis-mono"
                  style={{
                    backgroundColor: config.monoFont === font ? palette.surfaceSelected : palette.surfaceCard,
                    borderColor: config.monoFont === font ? palette.accentPrimary : palette.surfaceBorder,
                  }}
                >
                  {font}
                </button>
              ))}
            </div>
          </div>
        )}

        {section === 'sound' && (
          <div className="space-y-4">
            <h3 className="text-[14px] font-semibold">Intel AC&apos;97 Audio Mixer</h3>
            <div className="p-3 rounded border space-y-3" style={{ backgroundColor: palette.surfaceCard, borderColor: palette.surfaceBorder }}>
              <div className="flex items-center justify-between">
                <span>Master Volume</span>
                <span className="amaryllis-mono">{audioState.volume}%</span>
              </div>
              <input
                type="range"
                min={0}
                max={100}
                value={audioState.volume}
                onChange={(e) => {
                  ac97Mixer.setVolume(Number(e.target.value));
                  setAudioState(ac97Mixer.getState());
                }}
                className="w-full accent-[#24A87E]"
              />
              <button
                onClick={() => ac97Mixer.beep(587, 120)}
                className="px-3 py-1.5 rounded font-medium cursor-pointer"
                style={{ backgroundColor: palette.accentPrimary, color: palette.onAccent }}
              >
                Test AC&apos;97 Chime (587 Hz)
              </button>
            </div>
          </div>
        )}

        {section === 'network' && (
          <div className="space-y-3">
            <h3 className="text-[14px] font-semibold">Intel 82540EM (e1000) NIC</h3>
            <div className="p-3 rounded border space-y-1.5 amaryllis-mono text-[11.5px]" style={{ backgroundColor: palette.surfaceCard, borderColor: palette.surfaceBorder }}>
              <div>Interface: {net.nicName}</div>
              <div>Link Status: {net.linkUp ? 'UP (1000 Mbps Full Duplex)' : 'DOWN'}</div>
              <div>MAC: {net.mac}</div>
              <div>IPv4: {net.ip} / 24</div>
              <div>Gateway: {net.gateway}</div>
              <div>DNS: {net.dns}</div>
            </div>
            <button
              onClick={() => anuKernel.acquireDhcp()}
              className="px-3 py-1.5 rounded font-medium cursor-pointer"
              style={{ backgroundColor: palette.accentPrimary, color: palette.onAccent }}
            >
              Renew DHCP Lease
            </button>
          </div>
        )}
      </div>
    </div>
  );
};
