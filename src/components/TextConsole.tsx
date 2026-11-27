import React, { useEffect, useRef, useState } from 'react';
import { executeShellCommand, ShellSpan } from '../os/shell';
import { vfs } from '../os/vfs';

interface TextConsoleProps {
  onLaunchGui: () => void;
}

interface ConsoleLine {
  id: number;
  spans: ShellSpan[];
}

export const TextConsole: React.FC<TextConsoleProps> = ({ onLaunchGui }) => {
  const [lines, setLines] = useState<ConsoleLine[]>([
    {
      id: 1,
      spans: [{ text: 'AmaryllisOS VGA/Framebuffer Text Console (ANU v0.11 x86_64)\n', color: '#28B478' }],
    },
    {
      id: 2,
      spans: [{ text: "Desktop compositor exited cleanly. Type 'gui' or 'startx' to relaunch desktop, or 'help'.\n\n", color: '#9AA7BC' }],
    },
  ]);
  const [input, setInput] = useState('');
  const [cwd, setCwd] = useState(vfs.getCwd());
  const nextId = useRef(3);
  const inputRef = useRef<HTMLInputElement | null>(null);
  const scrollRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (scrollRef.current) scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
  }, [lines]);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    const raw = input;
    setInput('');
    setLines((prev) => [
      ...prev,
      {
        id: nextId.current++,
        spans: [
          { text: 'amaryllis@localhost:', color: '#28B478' },
          { text: vfs.getCwd(), color: '#DCBE5A' },
          { text: '$ ', color: '#28B478' },
          { text: raw + '\n', color: '#EAEFF7' },
        ],
      },
    ]);

    const res = await executeShellCommand(raw);
    setCwd(vfs.getCwd());
    if (res.clear) {
      setLines([]);
      return;
    }
    if (res.spans.length > 0) {
      setLines((prev) => [...prev, { id: nextId.current++, spans: res.spans }]);
    }
    if (res.action?.type === 'launch_gui') {
      onLaunchGui();
    }
  };

  return (
    <div
      className="fixed inset-0 bg-[#050608] text-[#EAEFF7] amaryllis-mono text-[14px] flex flex-col p-5 cursor-text"
      onClick={() => inputRef.current?.focus()}
    >
      <div className="h-1.5 w-full bg-[#28B478] mb-4 rounded-sm" />
      <div ref={scrollRef} className="flex-1 overflow-y-auto whitespace-pre-wrap break-words">
        {lines.map((l) => (
          <span key={l.id}>
            {l.spans.map((s, i) => (
              <span key={i} style={{ color: s.color }}>
                {s.text}
              </span>
            ))}
          </span>
        ))}
        <form onSubmit={handleSubmit} className="flex items-center mt-1">
          <span className="text-[#28B478]">amaryllis@localhost:</span>
          <span className="text-[#DCBE5A]">{cwd}</span>
          <span className="text-[#28B478]">$&nbsp;</span>
          <input
            ref={inputRef}
            autoFocus
            value={input}
            onChange={(e) => setInput(e.target.value)}
            className="flex-1 bg-transparent outline-none text-[#EAEFF7] caret-[#28B478]"
            spellCheck={false}
          />
        </form>
      </div>
      <div className="pt-2 border-t border-white/10 flex justify-between text-[12px] text-[#8C94A4]">
        <span>amsh • Raw Framebuffer Console</span>
        <button
          type="button"
          onClick={onLaunchGui}
          className="px-3 py-1 rounded bg-[#28B478] text-[#050608] font-semibold cursor-pointer"
        >
          Launch Amaryllis Desktop (gui)
        </button>
      </div>
    </div>
  );
};
