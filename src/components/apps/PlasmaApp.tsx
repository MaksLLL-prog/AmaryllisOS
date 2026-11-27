import React, { useEffect, useRef, useState } from 'react';
import { ac97Mixer } from '../../os/audio';
import { anuKernel } from '../../os/kernel';
import { ThemePalette } from '../../os/theme';

interface PlasmaAppProps {
  pid: number;
  palette: ThemePalette;
  onClose: () => void;
}

const WIN_W = 360;
const WIN_H = 240;

export const PlasmaApp: React.FC<PlasmaAppProps> = ({ pid, palette, onClose }) => {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const [marker, setMarker] = useState({ x: WIN_W / 2, y: WIN_H / 2 });
  const markerRef = useRef(marker);
  markerRef.current = marker;

  useEffect(() => {
    let rafId = 0;
    let phase = 0;
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext('2d');
    if (!ctx) return;
    const img = ctx.createImageData(WIN_W, WIN_H);

    const render = () => {
      const data = img.data;
      for (let y = 0; y < WIN_H; y++) {
        const wy = (y + phase) & 255;
        for (let x = 0; x < WIN_W; x++) {
          const wx = (x + (phase >> 1)) & 255;
          const idx = (y * WIN_W + x) * 4;
          data[idx] = ((wx ^ wy) & 0x7f) + 40;
          data[idx + 1] = ((x + y + phase) & 0x9f) + 50;
          data[idx + 2] = 140 + ((wx * 3 + wy) & 0x5f);
          data[idx + 3] = 255;
        }
      }
      ctx.putImageData(img, 0, 0);

      // Interactive cursor beacon drawn by Ring 3 userland (matching gui_plasma.rs)
      const m = markerRef.current;
      ctx.beginPath();
      ctx.arc(m.x, m.y, 16, 0, Math.PI * 2);
      ctx.fillStyle = '#FFFFFF';
      ctx.fill();
      ctx.beginPath();
      ctx.arc(m.x, m.y, 11, 0, Math.PI * 2);
      ctx.fillStyle = '#3480FF';
      ctx.fill();

      phase = (phase + 2) & 0xffff;
      rafId = requestAnimationFrame(render);
    };

    rafId = requestAnimationFrame(render);
    return () => cancelAnimationFrame(rafId);
  }, []);

  return (
    <div
      className="flex flex-col h-full w-full items-center justify-between p-2 outline-none"
      style={{ backgroundColor: palette.winBgActive, color: palette.textPrimary }}
      tabIndex={0}
      onKeyDown={(e) => {
        if (e.key.toLowerCase() === 'q') {
          anuKernel.killProcess(pid);
          onClose();
        }
      }}
    >
      <canvas
        ref={canvasRef}
        width={WIN_W}
        height={WIN_H}
        onClick={(e) => {
          const rect = e.currentTarget.getBoundingClientRect();
          const x = Math.round(((e.clientX - rect.left) / rect.width) * WIN_W);
          const y = Math.round(((e.clientY - rect.top) / rect.height) * WIN_H);
          setMarker({ x, y });
          ac97Mixer.beep(740, 45);
        }}
        className="w-full flex-1 rounded cursor-crosshair object-contain"
      />
      <div className="w-full pt-1.5 flex items-center justify-between text-[11px]" style={{ color: palette.textSecondary }}>
        <span>Ring 3 SHM Window • PID {pid} • Click to beacon &amp; beep</span>
        <span>Press &apos;q&apos; to exit</span>
      </div>
    </div>
  );
};
