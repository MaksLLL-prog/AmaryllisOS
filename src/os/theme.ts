export type ThemeMode = 'dark' | 'light';
export type SurfaceStyle = 'standard' | 'midnight' | 'snow';

export interface AccentOption {
  id: string;
  label: string;
  color: string;
  lightColor?: string;
}

export const ACCENT_PRESETS: AccentOption[] = [
  { id: 'mono', label: 'Mono', color: '#EAEAF0', lightColor: '#18181D' },
  { id: 'blue', label: 'Blue', color: '#2F80ED' },
  { id: 'cyan', label: 'Cyan', color: '#06B6D4' },
  { id: 'green', label: 'Green', color: '#10B981' },
  { id: 'teal', label: 'Teal', color: '#14B8A6' },
  { id: 'orange', label: 'Orange', color: '#F97316' },
  { id: 'red', label: 'Red', color: '#EF4444' },
  { id: 'slate', label: 'Slate', color: '#71717A' },
];

export type WallpaperPresetId =
  | 'monochrome'
  | 'silk_dunes'
  | 'aurora'
  | 'sunset'
  | 'midnight';

export interface WallpaperPresetInfo {
  id: WallpaperPresetId;
  title: string;
  shortTitle: string;
  preview: [string, string, string];
}

export const WALLPAPER_PRESETS: WallpaperPresetInfo[] = [
  {
    id: 'monochrome',
    title: 'Monochrome Silk',
    shortTitle: 'Monochrome',
    preview: ['#0A0A0D', '#1A1A20', '#E4E4EA'],
  },
  {
    id: 'silk_dunes',
    title: 'Silk Dunes',
    shortTitle: 'Silk Dunes',
    preview: ['#334D70', '#9C7662', '#F3C6A2'],
  },
  {
    id: 'aurora',
    title: 'Aurora Borealis',
    shortTitle: 'Aurora',
    preview: ['#061622', '#0F4C50', '#34E8B6'],
  },
  {
    id: 'sunset',
    title: 'Sunset Horizon',
    shortTitle: 'Sunset',
    preview: ['#1C1418', '#8C3648', '#F49D60'],
  },
  {
    id: 'midnight',
    title: 'Midnight Minimal',
    shortTitle: 'Carbon',
    preview: ['#0B0C10', '#171920', '#4A505E'],
  },
];

export interface ThemePalette {
  isLight: boolean;
  topbarBg: string;
  topbarBorder: string;
  topbarText: string;
  topbarMuted: string;
  dockBg: string;
  dockBorder: string;
  dockDivider: string;
  dockDot: string;
  modalBg: string;
  modalBorder: string;
  modalInputBg: string;
  modalHover: string;
  winBgActive: string;
  winBgInactive: string;
  winTitleActive: string;
  winTitleInactive: string;
  winBorderActive: string;
  winBorderInactive: string;
  winDivider: string;
  surfacePanel: string;
  surfaceSidebar: string;
  surfaceCard: string;
  surfaceToolbar: string;
  surfaceBorder: string;
  surfaceSelected: string;
  surfaceHover: string;
  textPrimary: string;
  textSecondary: string;
  textMuted: string;
  accentPrimary: string;
  onAccent: string;
  accentMint: string;
  accentCyan: string;
  accentWarn: string;
  accentErr: string;
}

function hexToRgb(hex: string): [number, number, number] {
  const clean = hex.replace('#', '');
  const num = parseInt(clean, 16);
  return [(num >> 16) & 255, (num >> 8) & 255, num & 255];
}

function rgbToHex(r: number, g: number, b: number): string {
  const toHex = (n: number) => Math.max(0, Math.min(255, Math.round(n))).toString(16).padStart(2, '0');
  return `#${toHex(r)}${toHex(g)}${toHex(b)}`;
}

function mixHex(a: string, b: string, weight01: number): string {
  const [ar, ag, ab] = hexToRgb(a);
  const [br, bg, bb] = hexToRgb(b);
  const inv = 1 - weight01;
  return rgbToHex(ar * inv + br * weight01, ag * inv + bg * weight01, ab * inv + bb * weight01);
}

function contrastOn(hex: string): string {
  const [r, g, b] = hexToRgb(hex);
  const luma = (r * 77 + g * 150 + b * 29) >> 8;
  return luma > 152 ? '#0B0B0E' : '#FFFFFF';
}

export function derivePalette(
  mode: ThemeMode,
  surfaceStyle: SurfaceStyle,
  accentId: string
): ThemePalette {
  const preset = ACCENT_PRESETS.find((p) => p.id === accentId) ?? ACCENT_PRESETS[0];
  const accent = mode === 'light' && preset.lightColor ? preset.lightColor : preset.color;

  const base: ThemePalette =
    mode === 'light'
      ? {
          isLight: true,
          topbarBg: 'rgba(245, 246, 248, 0.86)',
          topbarBorder: '#D4D7DF',
          topbarText: '#121216',
          topbarMuted: '#5A5D66',
          dockBg: 'rgba(242, 243, 246, 0.84)',
          dockBorder: '#FFFFFF',
          dockDivider: '#B8BDC7',
          dockDot: '#141418',
          modalBg: 'rgba(247, 248, 250, 0.95)',
          modalBorder: '#C2C7D0',
          modalInputBg: '#FFFFFF',
          modalHover: '#E6E8ED',
          winBgActive: '#FFFFFF',
          winBgInactive: '#F4F5F8',
          winTitleActive: '#F1F2F5',
          winTitleInactive: '#E8EAEE',
          winBorderActive: '#C4C8D2',
          winBorderInactive: '#D8DCE4',
          winDivider: '#E0E3EA',
          surfacePanel: '#FFFFFF',
          surfaceSidebar: '#F1F2F5',
          surfaceCard: '#F5F6F9',
          surfaceToolbar: '#F6F7FA',
          surfaceBorder: '#D8DCE4',
          surfaceSelected: '#E2E5EC',
          surfaceHover: '#ECEEF3',
          textPrimary: '#111216',
          textSecondary: '#4E525C',
          textMuted: '#7D828E',
          accentPrimary: accent,
          onAccent: contrastOn(accent),
          accentMint: '#1D9A72',
          accentCyan: '#2884E6',
          accentWarn: '#D99018',
          accentErr: '#DE4B5A',
        }
      : {
          isLight: false,
          topbarBg: 'rgba(10, 10, 13, 0.84)',
          topbarBorder: '#23232A',
          topbarText: '#F4F4F7',
          topbarMuted: '#9898A4',
          dockBg: 'rgba(14, 14, 18, 0.82)',
          dockBorder: '#2D2D36',
          dockDivider: '#2D2D36',
          dockDot: '#FFFFFF',
          modalBg: 'rgba(17, 17, 21, 0.95)',
          modalBorder: '#2E2E38',
          modalInputBg: '#09090C',
          modalHover: '#22222A',
          winBgActive: '#0C0C10',
          winBgInactive: '#101014',
          winTitleActive: '#141419',
          winTitleInactive: '#101014',
          winBorderActive: '#34343F',
          winBorderInactive: '#22222A',
          winDivider: '#212128',
          surfacePanel: '#0B0B0F',
          surfaceSidebar: '#0E0E13',
          surfaceCard: '#14141A',
          surfaceToolbar: '#121218',
          surfaceBorder: '#26262F',
          surfaceSelected: '#282832',
          surfaceHover: '#1D1D25',
          textPrimary: '#F4F4F7',
          textSecondary: '#AEAEB8',
          textMuted: '#6E6E7A',
          accentPrimary: accent,
          onAccent: contrastOn(accent),
          accentMint: '#24A87E',
          accentCyan: '#2884E6',
          accentWarn: '#D99018',
          accentErr: '#DE4B5A',
        };

  if (mode === 'dark' && surfaceStyle === 'midnight') {
    base.topbarBg = 'rgba(5, 5, 7, 0.9)';
    base.topbarBorder = '#1A1A20';
    base.dockBg = 'rgba(7, 7, 9, 0.88)';
    base.winBgActive = '#060608';
    base.winTitleActive = '#0C0C10';
    base.surfacePanel = '#050507';
    base.surfaceSidebar = '#08080B';
    base.surfaceCard = '#0E0E13';
    base.surfaceToolbar = '#0B0B0F';
    base.surfaceBorder = '#1F1F27';
  } else if (mode === 'light' && surfaceStyle === 'snow') {
    base.topbarBg = 'rgba(255, 255, 255, 0.92)';
    base.dockBg = 'rgba(250, 250, 252, 0.9)';
    base.surfaceSidebar = '#F5F6F9';
    base.surfaceCard = '#FAFAFC';
    base.surfaceToolbar = '#FAFAFC';
  }

  if (mode === 'light') {
    base.surfaceSelected = mixHex(base.surfaceCard, accent, 0.18);
    base.surfaceHover = mixHex(base.surfaceCard, accent, 0.09);
    base.winBorderActive = mixHex(base.winBorderActive, accent, 0.22);
    base.dockDot = mixHex('#141418', accent, 0.56);
  } else {
    base.surfaceSelected = mixHex(base.surfaceCard, accent, 0.24);
    base.surfaceHover = mixHex(base.surfaceCard, accent, 0.12);
    base.winBorderActive = mixHex(base.winBorderActive, accent, 0.28);
    base.dockDot = mixHex('#FFFFFF', accent, 0.7);
  }

  return base;
}

export interface DesktopConfig {
  themeMode: ThemeMode;
  surfaceStyle: SurfaceStyle;
  accentId: string;
  wallpaper: WallpaperPresetId;
  showClock: boolean;
  showDate: boolean;
  clock24h: boolean;
  dockAutoHide: boolean;
  animations: boolean;
  monoFont: 'JetBrainsMono' | 'AmaryllisMono';
}

export const DEFAULT_DESKTOP_CONFIG: DesktopConfig = {
  themeMode: 'dark',
  surfaceStyle: 'standard',
  accentId: 'mono',
  wallpaper: 'monochrome',
  showClock: true,
  showDate: true,
  clock24h: true,
  dockAutoHide: false,
  animations: true,
  monoFont: 'JetBrainsMono',
};
