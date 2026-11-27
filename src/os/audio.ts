export type BuiltinTrackId = 'startup_chime' | 'ambient_arp' | 'reference_440';

export interface AudioTrack {
  id: string;
  title: string;
  subtitle: string;
  durationSec: number;
  kind: 'builtin' | 'wav';
  builtinId?: BuiltinTrackId;
  path?: string;
}

class Ac97AudioMixer {
  private ctx: AudioContext | null = null;
  private gainNode: GainNode | null = null;
  private volume = 0.72;
  private muted = false;
  private playing = false;
  private currentTrack: AudioTrack | null = null;
  private startedAt = 0;
  private timerHandle: number | null = null;
  private activeOscs: OscillatorNode[] = [];
  private listeners = new Set<() => void>();

  private ensureContext(): AudioContext | null {
    if (typeof window === 'undefined') return null;
    if (!this.ctx) {
      const Ctx = window.AudioContext || (window as unknown as { webkitAudioContext: typeof AudioContext }).webkitAudioContext;
      if (!Ctx) return null;
      this.ctx = new Ctx();
      this.gainNode = this.ctx.createGain();
      this.gainNode.gain.value = this.muted ? 0 : this.volume;
      this.gainNode.connect(this.ctx.destination);
    }
    if (this.ctx.state === 'suspended') {
      void this.ctx.resume();
    }
    return this.ctx;
  }

  subscribe(fn: () => void): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  private emit() {
    this.listeners.forEach((fn) => fn());
  }

  getState() {
    const elapsed = this.playing ? (performance.now() - this.startedAt) / 1000 : 0;
    return {
      playing: this.playing,
      volume: Math.round(this.volume * 100),
      muted: this.muted,
      currentTrack: this.currentTrack,
      elapsedSec: this.currentTrack ? Math.min(elapsed, this.currentTrack.durationSec) : 0,
    };
  }

  setVolume(pct: number) {
    this.volume = Math.max(0, Math.min(1, pct / 100));
    if (this.gainNode && this.ctx) {
      this.gainNode.gain.setTargetAtTime(this.muted ? 0 : this.volume, this.ctx.currentTime, 0.02);
    }
    this.emit();
  }

  toggleMute() {
    this.muted = !this.muted;
    if (this.gainNode && this.ctx) {
      this.gainNode.gain.setTargetAtTime(this.muted ? 0 : this.volume, this.ctx.currentTime, 0.02);
    }
    this.emit();
  }

  beep(freq = 587, durationMs = 110) {
    const ctx = this.ensureContext();
    if (!ctx || !this.gainNode) return;
    const osc = ctx.createOscillator();
    const env = ctx.createGain();
    osc.type = 'sine';
    osc.frequency.value = Math.max(80, Math.min(4000, freq));
    const now = ctx.currentTime;
    const dur = Math.max(0.03, Math.min(2, durationMs / 1000));
    env.gain.setValueAtTime(0.001, now);
    env.gain.exponentialRampToValueAtTime(0.28, now + 0.015);
    env.gain.exponentialRampToValueAtTime(0.0001, now + dur);
    osc.connect(env);
    env.connect(this.gainNode);
    osc.start(now);
    osc.stop(now + dur + 0.02);
  }

  stop() {
    if (this.timerHandle !== null) {
      window.clearTimeout(this.timerHandle);
      this.timerHandle = null;
    }
    for (const osc of this.activeOscs) {
      try {
        osc.stop();
      } catch {
        // ignore stopped oscillator
      }
    }
    this.activeOscs = [];
    this.playing = false;
    this.emit();
  }

  playTrack(track: AudioTrack) {
    this.stop();
    const ctx = this.ensureContext();
    this.currentTrack = track;
    this.playing = true;
    this.startedAt = performance.now();
    this.emit();

    if (ctx && this.gainNode) {
      const now = ctx.currentTime;
      const playNote = (freq: number, offsetSec: number, lenSec: number, type: OscillatorType = 'triangle', amp = 0.2) => {
        if (!this.gainNode) return;
        const osc = ctx.createOscillator();
        const env = ctx.createGain();
        osc.type = type;
        osc.frequency.value = freq;
        env.gain.setValueAtTime(0.0001, now + offsetSec);
        env.gain.exponentialRampToValueAtTime(amp, now + offsetSec + 0.03);
        env.gain.exponentialRampToValueAtTime(0.0001, now + offsetSec + lenSec);
        osc.connect(env);
        env.connect(this.gainNode);
        osc.start(now + offsetSec);
        osc.stop(now + offsetSec + lenSec + 0.02);
        this.activeOscs.push(osc);
      };

      if (track.builtinId === 'startup_chime' || track.title.includes('startup')) {
        // Warm Amaryllis startup chord: D4 -> F#4 -> A4 -> C#5 -> E5
        [293.66, 369.99, 440.0, 554.37, 659.25].forEach((f, idx) => {
          playNote(f, idx * 0.14, 1.5 - idx * 0.1, 'sine', 0.16);
        });
      } else if (track.builtinId === 'reference_440') {
        playNote(440, 0, track.durationSec, 'sine', 0.18);
      } else {
        // Ambient Arp / Theme melody
        const notes = [261.63, 329.63, 392.0, 493.88, 523.25, 493.88, 392.0, 329.63];
        for (let i = 0; i < 16; i++) {
          playNote(notes[i % notes.length], i * 0.36, 0.34, 'triangle', 0.15);
        }
      }
    }

    this.timerHandle = window.setTimeout(() => {
      this.playing = false;
      this.activeOscs = [];
      this.emit();
    }, track.durationSec * 1000);
  }
}

export const ac97Mixer = new Ac97AudioMixer();
