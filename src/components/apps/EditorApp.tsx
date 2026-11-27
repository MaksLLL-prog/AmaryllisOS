import React, { useEffect, useState } from 'react';
import { FolderOpen, Save } from 'lucide-react';
import { ThemePalette } from '../../os/theme';
import { vfs } from '../../os/vfs';

interface EditorAppProps {
  palette: ThemePalette;
  initialPath?: string;
}

export const EditorApp: React.FC<EditorAppProps> = ({ palette, initialPath = '/home/notes.txt' }) => {
  const [filePath, setFilePath] = useState(initialPath);
  const [pathInput, setPathInput] = useState(initialPath);
  const [content, setContent] = useState('');
  const [modified, setModified] = useState(false);
  const [statusMsg, setStatusMsg] = useState('Ready • Ctrl+S to save');

  const loadFile = (path: string) => {
    const abs = vfs.resolvePath(path);
    setFilePath(abs);
    setPathInput(abs);
    try {
      const data = vfs.readFile(abs);
      setContent(data);
      setModified(false);
      setStatusMsg(`Loaded ${abs} (${data.length} bytes)`);
    } catch {
      setContent('');
      setModified(false);
      setStatusMsg(`New buffer: ${abs} (Ctrl+S to create)`);
    }
  };

  useEffect(() => {
    loadFile(initialPath);
  }, [initialPath]);

  const handleSave = () => {
    try {
      vfs.writeFile(filePath, content);
      setModified(false);
      setStatusMsg(`Saved ${filePath} (${content.length} bytes)`);
    } catch (err) {
      setStatusMsg(err instanceof Error ? err.message : 'Save failed');
    }
  };

  const lines = content.split('\n');

  return (
    <div
      className="flex flex-col h-full w-full text-[12px]"
      style={{ backgroundColor: palette.winBgActive, color: palette.textPrimary }}
      onKeyDown={(e) => {
        if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 's') {
          e.preventDefault();
          handleSave();
        }
      }}
    >
      <div
        className="flex items-center gap-2 px-3 py-1.5 border-b"
        style={{ backgroundColor: palette.surfaceToolbar, borderColor: palette.surfaceBorder }}
      >
        <input
          value={pathInput}
          onChange={(e) => setPathInput(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') loadFile(pathInput);
          }}
          className="flex-1 px-2.5 py-1 rounded border outline-none amaryllis-mono text-[12px]"
          style={{
            backgroundColor: palette.modalInputBg,
            borderColor: palette.surfaceBorder,
            color: palette.textPrimary,
          }}
        />
        <button
          onClick={() => loadFile(pathInput)}
          className="px-2.5 py-1 rounded border flex items-center gap-1 cursor-pointer"
          style={{ backgroundColor: palette.surfaceCard, borderColor: palette.surfaceBorder }}
        >
          <FolderOpen size={13} /> Open
        </button>
        <button
          onClick={handleSave}
          className="px-3 py-1 rounded flex items-center gap-1 font-medium cursor-pointer"
          style={{ backgroundColor: palette.accentPrimary, color: palette.onAccent }}
        >
          <Save size={13} /> Save{modified ? ' *' : ''}
        </button>
      </div>

      <div className="flex flex-1 min-h-0 overflow-hidden">
        <div
          className="w-11 py-2.5 px-2 text-right select-none amaryllis-mono text-[12px] leading-[1.5] border-r overflow-hidden"
          style={{
            backgroundColor: palette.surfaceSidebar,
            borderColor: palette.surfaceBorder,
            color: palette.textMuted,
          }}
        >
          {lines.map((_, idx) => (
            <div key={idx}>{idx + 1}</div>
          ))}
        </div>
        <textarea
          value={content}
          onChange={(e) => {
            setContent(e.target.value);
            setModified(true);
          }}
          spellCheck={false}
          className="flex-1 p-2.5 outline-none resize-none amaryllis-mono text-[12.5px] leading-[1.5]"
          style={{
            backgroundColor: palette.winBgActive,
            color: palette.textPrimary,
          }}
        />
      </div>

      <div
        className="px-3 py-1 border-t text-[11px] flex justify-between"
        style={{ backgroundColor: palette.surfaceToolbar, borderColor: palette.surfaceBorder, color: palette.textMuted }}
      >
        <span>{statusMsg}</span>
        <span>
          {lines.length} lines • {content.length} bytes
        </span>
      </div>
    </div>
  );
};
