import React, { useEffect, useRef, useState } from 'react';
import { anuKernel } from '../../os/kernel';
import { executeShellCommand, ShellSpan } from '../../os/shell';
import { ThemePalette } from '../../os/theme';
import { vfs } from '../../os/vfs';

interface TerminalLine {
  id: number;
  spans: ShellSpan[];
}

interface TerminalAppProps {
  palette: ThemePalette;
  pendingCommand?: string | null;
  onConsumePendingCommand?: () => void;
  onExitDesktop: () => void;
  onOpenPlasma: (pid: number) => void;
}

const KNOWN_COMMANDS = [
  'help', 'clear', 'about', 'version', 'echo', 'whoami', 'neofetch', 'uptime',
  'heap', 'pmm', 'tasks', 'processes', 'lspci', 'drivers', 'ioreg', 'acpi',
  'apic', 'smp', 'klog', 'dmesg', 'kthread', 'ipc', 'ls', 'cd', 'pwd', 'cat',
  'write', 'mkdir', 'rm', 'diskinfo', 'mount', 'sync', 'netinfo', 'dhcp',
  'ping', 'httpget', 'soundinfo', 'beep', 'play', 'mkelf', 'elfinfo', 'exec',
  'execbg', 'ps', 'killr3', 'gui', 'startx', 'exit',
];

export const TerminalApp: React.FC<TerminalAppProps> = ({
  palette,
  pendingCommand,
  onConsumePendingCommand,
  onExitDesktop,
  onOpenPlasma,
}) => {
  const [lines, setLines] = useState<TerminalLine[]>([
    {
      id: 1,
      spans: [{ text: 'AmaryllisOS Interactive Shell (GUI Session)\n', color: '#24A87E' }],
    },
    {
      id: 2,
      spans: [
        {
          text: "Type 'help', 'neofetch', 'ls', 'diskinfo', 'netinfo', 'mkelf', 'exec /gui_demo.elf', or 'exit'.\n",
          color: '#9AA7BC',
        },
      ],
    },
  ]);
  const [input, setInput] = useState('');
  const [history, setHistory] = useState<string[]>([]);
  const [histIdx, setHistIdx] = useState(-1);
  const [cwd, setCwd] = useState(vfs.getCwd());
  const [busy, setBusy] = useState(false);

  const nextId = useRef(3);
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const inputRef = useRef<HTMLInputElement | null>(null);

  useEffect(() => {
    return vfs.subscribe(() => setCwd(vfs.getCwd()));
  }, []);

  useEffect(() => {
    if (scrollRef.current) {
      scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
    }
  }, [lines]);

  const runCommand = async (raw: string) => {
    const promptLine: TerminalLine = {
      id: nextId.current++,
      spans: [
        { text: 'amaryllis:', color: '#24A87E' },
        { text: vfs.getCwd(), color: '#D99018' },
        { text: '$ ', color: '#24A87E' },
        { text: raw + '\n', color: '#EAEFF7' },
      ],
    };
    setLines((prev) => [...prev.slice(-280), promptLine]);

    if (raw.trim()) {
      setHistory((prev) => [raw, ...prev.slice(0, 49)]);
      setHistIdx(-1);
    }

    setBusy(true);
    const res = await executeShellCommand(raw);
    setBusy(false);
    setCwd(vfs.getCwd());

    if (res.clear) {
      setLines([]);
      return;
    }

    if (res.spans.length > 0) {
      setLines((prev) => [...prev.slice(-280), { id: nextId.current++, spans: res.spans }]);
    }

    if (res.action?.type === 'exit_gui') {
      onExitDesktop();
    } else if (res.action?.type === 'open_plasma') {
      onOpenPlasma(res.action.pid);
    }
  };

  useEffect(() => {
    if (pendingCommand) {
      void runCommand(pendingCommand);
      onConsumePendingCommand?.();
    }
  }, [pendingCommand]);

  const handleKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'Enter' && !busy) {
      e.preventDefault();
      const cmd = input;
      setInput('');
      void runCommand(cmd);
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      if (history.length > 0) {
        const next = Math.min(history.length - 1, histIdx + 1);
        setHistIdx(next);
        setInput(history[next]);
      }
    } else if (e.key === 'ArrowDown') {
      e.preventDefault();
      if (histIdx <= 0) {
        setHistIdx(-1);
        setInput('');
      } else {
        const next = histIdx - 1;
        setHistIdx(next);
        setInput(history[next]);
      }
    } else if (e.key === 'Tab') {
      e.preventDefault();
      const token = input.trim();
      if (!token) return;
      if (!token.includes(' ')) {
        const match = KNOWN_COMMANDS.find((c) => c.startsWith(token));
        if (match) setInput(match + ' ');
      } else {
        const parts = input.split(' ');
        const last = parts[parts.length - 1];
        try {
          const entries = vfs.listDir(vfs.getCwd());
          const match = entries.find((item) => item.name.startsWith(last.replace(/^\//, '')));
          if (match) {
            parts[parts.length - 1] = last.startsWith('/') ? `/${match.name}` : match.name;
            setInput(parts.join(' '));
          }
        } catch {
          // ignore
        }
      }
    } else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'l') {
      e.preventDefault();
      setLines([]);
    } else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'v') {
      const clip = anuKernel.getClipboard();
      if (clip && !navigator.clipboard) {
        e.preventDefault();
        setInput((prev) => prev + clip);
      }
    }
  };

  return (
    <div
      className="flex flex-col h-full w-full amaryllis-mono text-[13px] leading-[1.45] cursor-text"
      style={{ backgroundColor: '#0A0D14', color: '#EAEFF7' }}
      onClick={() => inputRef.current?.focus()}
    >
      <div ref={scrollRef} className="flex-1 overflow-y-auto px-3.5 py-3 whitespace-pre-wrap break-words select-text">
        {lines.map((line) => (
          <span key={line.id}>
            {line.spans.map((sp, idx) => (
              <span key={idx} style={{ color: sp.color }}>
                {sp.text}
              </span>
            ))}
          </span>
        ))}
        <div className="flex items-center gap-0 mt-0.5">
          <span style={{ color: '#24A87E' }}>amaryllis:</span>
          <span style={{ color: '#D99018' }}>{cwd}</span>
          <span style={{ color: '#24A87E' }}>$&nbsp;</span>
          <input
            ref={inputRef}
            type="text"
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={handleKeyDown}
            disabled={busy}
            autoFocus
            spellCheck={false}
            className="flex-1 bg-transparent outline-none border-none text-[#EAEFF7] amaryllis-mono text-[13px] caret-[#24A87E]"
            placeholder={busy ? 'executing...' : ''}
          />
        </div>
      </div>
      <div
        className="px-3 py-1 text-[11px] flex items-center justify-between border-t"
        style={{
          backgroundColor: palette.surfaceToolbar,
          borderColor: palette.surfaceBorder,
          color: palette.textMuted,
        }}
      >
        <span>amsh • Ring 3 &amp; Kernel Diagnostics</span>
        <span>Tab: autocomplete • Ctrl+L: clear • exit: VGA shell</span>
      </div>
    </div>
  );
};
