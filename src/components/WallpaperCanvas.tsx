import React, { useEffect, useRef } from 'react';
import { ThemeMode, WALLPAPER_PRESETS, WallpaperPresetId } from '../os/theme';

interface WallpaperCanvasProps {
  preset: WallpaperPresetId;
  themeMode: ThemeMode;
}

export const WallpaperCanvas: React.FC<WallpaperCanvasProps> = ({ preset, themeMode }) => {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const draw = () => {
      const w = (canvas.width = window.innerWidth);
      const h = (canvas.height = window.innerHeight);
      const ctx = canvas.getContext('2d');
      if (!ctx) return;

      const info = WALLPAPER_PRESETS.find((p) => p.id === preset) ?? WALLPAPER_PRESETS[0];
      const [c0, c1, c2] = info.preview;

      // Base gradient adapted for light vs dark mode when monochrome is active
      const grad = ctx.createLinearGradient(0, 0, w, h);
      if (preset === 'monochrome' && themeMode === 'light') {
        grad.addColorStop(0, '#ECEEF2');
        grad.addColorStop(0.55, '#DDE1EA');
        grad.addColorStop(1, '#CFD5E2');
      } else {
        grad.addColorStop(0, c0);
        grad.addColorStop(0.55, c1);
        grad.addColorStop(1, c0);
      }
      ctx.fillStyle = grad;
      ctx.fillRect(0, 0, w, h);

      // 4-layer procedural silk waves (matching build_amaryllis_wallpaper)
      const waveConfigs = [
        { amp: h * 0.14, freq: 0.0024, yBase: h * 0.42, phase: 0.4, alpha: themeMode === 'light' && preset === 'monochrome' ? 0.18 : 0.16 },
        { amp: h * 0.18, freq: 0.0019, yBase: h * 0.56, phase: 1.7, alpha: themeMode === 'light' && preset === 'monochrome' ? 0.22 : 0.2 },
        { amp: h * 0.11, freq: 0.0031, yBase: h * 0.68, phase: 3.1, alpha: themeMode === 'light' && preset === 'monochrome' ? 0.26 : 0.24 },
        { amp: h * 0.09, freq: 0.0028, yBase: h * 0.78, phase: 4.5, alpha: themeMode === 'light' && preset === 'monochrome' ? 0.3 : 0.28 },
      ];

      for (const layer of waveConfigs) {
        ctx.beginPath();
        ctx.moveTo(0, h);
        for (let x = 0; x <= w; x += 12) {
          const y =
            layer.yBase +
            Math.sin(x * layer.freq + layer.phase) * layer.amp +
            Math.cos(x * layer.freq * 0.55 - layer.phase * 0.7) * (layer.amp * 0.45);
          ctx.lineTo(x, y);
        }
        ctx.lineTo(w, h);
        ctx.closePath();

        const waveGrad = ctx.createLinearGradient(0, layer.yBase - layer.amp, w, h);
        const waveHighlight = preset === 'monochrome' && themeMode === 'light' ? '#70798C' : c2;
        waveGrad.addColorStop(0, `${waveHighlight}${Math.round(layer.alpha * 255).toString(16).padStart(2, '0')}`);
        waveGrad.addColorStop(1, `${c1}14`);
        ctx.fillStyle = waveGrad;
        ctx.fill();

        ctx.strokeStyle = `${waveHighlight}33`;
        ctx.lineWidth = 1.25;
        ctx.stroke();
      }
    };

    draw();
    window.addEventListener('resize', draw);
    return () => window.removeEventListener('resize', draw);
  }, [preset, themeMode]);

  return <canvas ref={canvasRef} className="fixed inset-0 w-full h-full pointer-events-none" />;
};
