import React, { useEffect, useState } from 'react';
import { Music, Play, Square, Volume2, VolumeX } from 'lucide-react';
import { ac97Mixer, AudioTrack } from '../../os/audio';
import { ThemePalette } from '../../os/theme';
import { vfs } from '../../os/vfs';

interface AudioPlayerAppProps {
  palette: ThemePalette;
  initialWavPath?: string | null;
}

const BUILTIN_TRACKS: AudioTrack[] = [
  {
    id: 'startup_chime',
    title: 'Amaryllis Startup Chime',
    subtitle: 'Synth  |  2.4s  |  44.1 kHz Stereo',
    durationSec: 2.4,
    kind: 'builtin',
    builtinId: 'startup_chime',
  },
  {
    id: 'ambient_arp',
    title: 'Twilight Ambient Arp',
    subtitle: 'Synth  |  6.4s  |  44.1 kHz Stereo',
    durationSec: 6.4,
    kind: 'builtin',
    builtinId: 'ambient_arp',
  },
  {
    id: 'reference_440',
    title: 'Reference 440 Hz Tone',
    subtitle: 'Tone   |  3.0s  |  44.1 kHz Sine',
    durationSec: 3.0,
    kind: 'builtin',
    builtinId: 'reference_440',
  },
];

export const AudioPlayerApp: React.FC<AudioPlayerAppProps> = ({ palette, initialWavPath }) => {
  const [tracks, setTracks] = useState<AudioTrack[]>(BUILTIN_TRACKS);
  const [selectedId, setSelectedId] = useState<string>('startup_chime');
  const [mixerState, setMixerState] = useState(ac97Mixer.getState());
  const [bars, setBars] = useState<number[]>(Array(32).fill(12));

  const scanTracks = () => {
    const wavs: AudioTrack[] = [];
    for (const dir of ['/disk/audio', '/disk', '/home', '/']) {
      try {
        for (const entry of vfs.listDir(dir)) {
          if (entry.kind === 'file' && entry.name.toLowerCase().endsWith('.wav')) {
            wavs.push({
              id: entry.path,
              title: entry.name,
              subtitle: `WAV    |  ${entry.path}  |  44.1 kHz`,
              durationSec: 5.8,
              kind: 'wav',
              path: entry.path,
            });
          }
        }
      } catch {
        // ignore missing dir
      }
    }
    setTracks([...BUILTIN_TRACKS, ...wavs]);
  };

  useEffect(() => {
    scanTracks();
    const unsubVfs = vfs.subscribe(scanTracks);
    const unsubMix = ac97Mixer.subscribe(() => setMixerState(ac97Mixer.getState()));
    return () => {
      unsubVfs();
      unsubMix();
    };
  }, []);

  useEffect(() => {
    if (initialWavPath) {
      setSelectedId(initialWavPath);
      ac97Mixer.playTrack({
        id: initialWavPath,
        title: initialWavPath.split('/').pop() || initialWavPath,
        subtitle: `WAV    |  ${initialWavPath}`,
        durationSec: 5.8,
        kind: 'wav',
        path: initialWavPath,
      });
    }
  }, [initialWavPath]);

  useEffect(() => {
    if (!mixerState.playing) {
      setBars(Array(32).fill(8));
      return;
    }
    const id = window.setInterval(() => {
      setMixerState(ac97Mixer.getState());
      setBars(
        Array.from({ length: 32 }, (_, idx) => {
          const wave = Math.sin(performance.now() * 0.008 + idx * 0.42) * 0.5 + 0.5;
          return Math.round(14 + wave * 78);
        })
      );
    }, 60);
    return () => window.clearInterval(id);
  }, [mixerState.playing]);

  const selectedTrack = tracks.find((t) => t.id === selectedId) ?? tracks[0];

  return (
    <div
      className="flex flex-col h-full w-full text-[12px]"
      style={{ backgroundColor: palette.winBgActive, color: palette.textPrimary }}
    >
      {/* Visualizer */}
      <div
        className="p-4 border-b flex flex-col gap-3"
        style={{ backgroundColor: palette.surfaceCard, borderColor: palette.surfaceBorder }}
      >
        <div className="flex items-center justify-between">
          <div>
            <div className="text-[14px] font-semibold">
              {mixerState.currentTrack?.title ?? selectedTrack?.title ?? 'Idle'}
            </div>
            <div className="text-[11px]" style={{ color: palette.textSecondary }}>
              Intel AC&apos;97 BDL DMA Mixer • {mixerState.playing ? 'Streaming 44.1 kHz' : 'Ready'}
            </div>
          </div>
          <div className="flex items-center gap-2">
            <button
              onClick={() => selectedTrack && ac97Mixer.playTrack(selectedTrack)}
              className="px-3 py-1.5 rounded font-medium flex items-center gap-1.5 cursor-pointer"
              style={{ backgroundColor: palette.accentMint, color: '#FFFFFF' }}
            >
              <Play size={13} /> Play
            </button>
            <button
              onClick={() => ac97Mixer.stop()}
              className="px-3 py-1.5 rounded border flex items-center gap-1.5 cursor-pointer"
              style={{ backgroundColor: palette.surfacePanel, borderColor: palette.surfaceBorder }}
            >
              <Square size={13} /> Stop
            </button>
          </div>
        </div>

        {/* 32-bar spectrum */}
        <div
          className="h-20 rounded border px-2.5 py-2 flex items-end gap-1"
          style={{ backgroundColor: palette.surfacePanel, borderColor: palette.surfaceBorder }}
        >
          {bars.map((h, i) => (
            <div
              key={i}
              className="flex-1 rounded-t transition-all duration-75"
              style={{
                height: `${h}%`,
                backgroundColor: mixerState.playing ? palette.accentMint : palette.surfaceBorder,
                opacity: mixerState.playing ? 0.88 : 0.5,
              }}
            />
          ))}
        </div>

        {/* Volume */}
        <div className="flex items-center gap-3">
          <button
            onClick={() => ac97Mixer.toggleMute()}
            className="p-1 rounded border cursor-pointer"
            style={{ borderColor: palette.surfaceBorder }}
          >
            {mixerState.muted ? <VolumeX size={14} /> : <Volume2 size={14} />}
          </button>
          <input
            type="range"
            min={0}
            max={100}
            value={mixerState.volume}
            onChange={(e) => ac97Mixer.setVolume(Number(e.target.value))}
            className="flex-1 accent-[#24A87E] cursor-pointer"
          />
          <span className="w-10 text-right amaryllis-mono">{mixerState.volume}%</span>
        </div>
      </div>

      {/* Playlist */}
      <div className="flex-1 overflow-y-auto p-3 space-y-1">
        <div className="text-[11px] uppercase tracking-wider mb-1.5" style={{ color: palette.textMuted }}>
          Tracks &amp; FAT32 WAV Streams
        </div>
        {tracks.map((track) => {
          const isSel = track.id === selectedId;
          return (
            <div
              key={track.id}
              onClick={() => setSelectedId(track.id)}
              onDoubleClick={() => ac97Mixer.playTrack(track)}
              className="flex items-center justify-between px-3 py-2 rounded cursor-pointer border"
              style={{
                backgroundColor: isSel ? palette.surfaceSelected : palette.surfaceCard,
                borderColor: isSel ? palette.accentPrimary : 'transparent',
              }}
            >
              <div className="flex items-center gap-2.5">
                <Music size={15} style={{ color: palette.accentMint }} />
                <div>
                  <div className="font-medium">{track.title}</div>
                  <div className="text-[11px]" style={{ color: palette.textSecondary }}>
                    {track.subtitle}
                  </div>
                </div>
              </div>
              <button
                onClick={(e) => {
                  e.stopPropagation();
                  setSelectedId(track.id);
                  ac97Mixer.playTrack(track);
                }}
                className="px-2.5 py-1 rounded text-[11px] border cursor-pointer"
                style={{ backgroundColor: palette.surfacePanel, borderColor: palette.surfaceBorder }}
              >
                Play
              </button>
            </div>
          );
        })}
      </div>
    </div>
  );
};
