import React, { useEffect, useState } from 'react';
import {
  ArrowLeft,
  Cpu,
  File as FileIcon,
  FilePlus,
  Folder,
  FolderPlus,
  HardDrive,
  Image,
  LayoutGrid,
  List,
  Music,
  Pencil,
  Trash2,
} from 'lucide-react';
import { ThemePalette } from '../../os/theme';
import { vfs, VfsDirEntry } from '../../os/vfs';

interface FilesAppProps {
  palette: ThemePalette;
  onOpenEditor: (path: string) => void;
  onExecElf: (path: string) => void;
  onPlayAudio: (path: string) => void;
}

export const FilesApp: React.FC<FilesAppProps> = ({
  palette,
  onOpenEditor,
  onExecElf,
  onPlayAudio,
}) => {
  const [currentPath, setCurrentPath] = useState('/');
  const [entries, setEntries] = useState<VfsDirEntry[]>([]);
  const [selected, setSelected] = useState<VfsDirEntry | null>(null);
  const [viewMode, setViewMode] = useState<'list' | 'grid'>('list');
  const [sortBy, setSortBy] = useState<'name' | 'size'>('name');
  const [promptMode, setPromptMode] = useState<'none' | 'newfile' | 'newdir' | 'rename'>('none');
  const [promptVal, setPromptVal] = useState('');
  const [statusMsg, setStatusMsg] = useState('Double-click to open • RAMFS / & FAT32 /disk');

  const loadDir = (path: string) => {
    try {
      const list = vfs.listDir(path);
      const sorted = [...list].sort((a, b) => {
        if (a.kind !== b.kind) return a.kind === 'directory' ? -1 : 1;
        return sortBy === 'name' ? a.name.localeCompare(b.name) : b.size - a.size;
      });
      setCurrentPath(path);
      setEntries(sorted);
      setSelected(null);
    } catch (err) {
      setStatusMsg(err instanceof Error ? err.message : 'Directory error');
    }
  };

  useEffect(() => {
    loadDir(currentPath);
    return vfs.subscribe(() => loadDir(currentPath));
  }, [currentPath, sortBy]);

  const handleOpen = (entry: VfsDirEntry) => {
    if (entry.kind === 'directory') {
      loadDir(entry.path);
      return;
    }
    if (entry.name.endsWith('.elf')) {
      onExecElf(entry.path);
      setStatusMsg(`Executed Ring 3 ELF: ${entry.path}`);
    } else if (entry.name.endsWith('.wav')) {
      onPlayAudio(entry.path);
      setStatusMsg(`Streaming WAV: ${entry.path}`);
    } else {
      onOpenEditor(entry.path);
    }
  };

  const handlePromptSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    const val = promptVal.trim();
    if (!val) {
      setPromptMode('none');
      return;
    }
    try {
      const target = currentPath === '/' ? `/${val}` : `${currentPath}/${val}`;
      if (promptMode === 'newfile') {
        vfs.writeFile(target, '');
        setStatusMsg(`Created ${target}`);
      } else if (promptMode === 'newdir') {
        vfs.createDir(target);
        setStatusMsg(`Created folder ${target}`);
      } else if (promptMode === 'rename' && selected) {
        vfs.rename(selected.path, val);
        setStatusMsg(`Renamed to ${val}`);
      }
      loadDir(currentPath);
    } catch (err) {
      setStatusMsg(err instanceof Error ? err.message : 'Operation failed');
    }
    setPromptMode('none');
    setPromptVal('');
  };

  const handleDelete = () => {
    if (!selected) return;
    try {
      vfs.remove(selected.path);
      setStatusMsg(`Deleted ${selected.path}`);
      loadDir(currentPath);
    } catch (err) {
      setStatusMsg(err instanceof Error ? err.message : 'Delete failed');
    }
  };

  const renderIcon = (entry: VfsDirEntry, size = 16) => {
    if (entry.kind === 'directory') return <Folder size={size} className="text-[#3890F0]" />;
    if (entry.name.endsWith('.elf')) return <Cpu size={size} className="text-[#24A87E]" />;
    if (entry.name.endsWith('.wav')) return <Music size={size} className="text-[#D99018]" />;
    if (entry.name.endsWith('.ppm') || entry.name.endsWith('.bmp')) {
      return <Image size={size} className="text-[#A855F7]" />;
    }
    return <FileIcon size={size} style={{ color: palette.textSecondary }} />;
  };

  const previewText = (() => {
    if (!selected) return null;
    if (selected.kind === 'directory') return `Directory (${selected.size} items) • ${selected.fsLabel}`;
    try {
      return vfs.readFile(selected.path).slice(0, 320);
    } catch {
      return '';
    }
  })();

  const breadcrumbs = ['/', ...currentPath.split('/').filter(Boolean)];

  return (
    <div className="flex flex-col h-full w-full text-[12px]" style={{ backgroundColor: palette.winBgActive, color: palette.textPrimary }}>
      {/* Toolbar */}
      <div
        className="flex items-center justify-between px-3 py-1.5 border-b gap-2"
        style={{ backgroundColor: palette.surfaceToolbar, borderColor: palette.surfaceBorder }}
      >
        <div className="flex items-center gap-1.5">
          <button
            onClick={() => {
              if (currentPath !== '/') {
                const parent = currentPath.slice(0, currentPath.lastIndexOf('/')) || '/';
                loadDir(parent);
              }
            }}
            className="px-2 py-1 rounded border flex items-center gap-1 cursor-pointer"
            style={{ backgroundColor: palette.surfaceCard, borderColor: palette.surfaceBorder }}
            title="Up one level"
          >
            <ArrowLeft size={13} /> Up
          </button>
          <button
            onClick={() => {
              setPromptMode('newfile');
              setPromptVal('untitled.txt');
            }}
            className="px-2 py-1 rounded border flex items-center gap-1 cursor-pointer"
            style={{ backgroundColor: palette.surfaceCard, borderColor: palette.surfaceBorder }}
          >
            <FilePlus size={13} /> + File
          </button>
          <button
            onClick={() => {
              setPromptMode('newdir');
              setPromptVal('new_folder');
            }}
            className="px-2 py-1 rounded border flex items-center gap-1 cursor-pointer"
            style={{ backgroundColor: palette.surfaceCard, borderColor: palette.surfaceBorder }}
          >
            <FolderPlus size={13} /> + Dir
          </button>
          {selected && (
            <>
              <button
                onClick={() => {
                  setPromptMode('rename');
                  setPromptVal(selected.name);
                }}
                className="px-2 py-1 rounded border flex items-center gap-1 cursor-pointer"
                style={{ backgroundColor: palette.surfaceCard, borderColor: palette.surfaceBorder }}
              >
                <Pencil size={13} /> Rename
              </button>
              <button
                onClick={handleDelete}
                className="px-2 py-1 rounded border flex items-center gap-1 cursor-pointer text-[#DE4B5A]"
                style={{ backgroundColor: palette.surfaceCard, borderColor: palette.surfaceBorder }}
              >
                <Trash2 size={13} /> Delete
              </button>
            </>
          )}
        </div>
        <div className="flex items-center gap-1.5">
          <button
            onClick={() => setSortBy((s) => (s === 'name' ? 'size' : 'name'))}
            className="px-2 py-1 rounded border cursor-pointer"
            style={{ backgroundColor: palette.surfaceCard, borderColor: palette.surfaceBorder }}
          >
            Sort: {sortBy === 'name' ? 'Name' : 'Size'}
          </button>
          <button
            onClick={() => setViewMode((v) => (v === 'list' ? 'grid' : 'list'))}
            className="p-1.5 rounded border cursor-pointer"
            style={{ backgroundColor: palette.surfaceCard, borderColor: palette.surfaceBorder }}
            title="Toggle List / Grid"
          >
            {viewMode === 'list' ? <LayoutGrid size={13} /> : <List size={13} />}
          </button>
        </div>
      </div>

      {/* Inline prompt bar */}
      {promptMode !== 'none' && (
        <form
          onSubmit={handlePromptSubmit}
          className="flex items-center gap-2 px-3 py-1.5 border-b"
          style={{ backgroundColor: palette.surfaceSelected, borderColor: palette.surfaceBorder }}
        >
          <span className="font-medium">
            {promptMode === 'newfile' ? 'New File:' : promptMode === 'newdir' ? 'New Folder:' : 'Rename:'}
          </span>
          <input
            autoFocus
            value={promptVal}
            onChange={(e) => setPromptVal(e.target.value)}
            className="flex-1 px-2 py-0.5 rounded border outline-none"
            style={{
              backgroundColor: palette.modalInputBg,
              borderColor: palette.surfaceBorder,
              color: palette.textPrimary,
            }}
          />
          <button type="submit" className="px-2.5 py-0.5 rounded font-medium cursor-pointer" style={{ backgroundColor: palette.accentPrimary, color: palette.onAccent }}>
            OK
          </button>
          <button type="button" onClick={() => setPromptMode('none')} className="px-2 py-0.5 rounded border cursor-pointer" style={{ borderColor: palette.surfaceBorder }}>
            Cancel
          </button>
        </form>
      )}

      {/* Breadcrumb */}
      <div
        className="flex items-center gap-1 px-3 py-1 border-b text-[11px]"
        style={{ backgroundColor: palette.surfacePanel, borderColor: palette.surfaceBorder, color: palette.textSecondary }}
      >
        {breadcrumbs.map((seg, idx) => {
          const target = idx === 0 ? '/' : '/' + breadcrumbs.slice(1, idx + 1).join('/');
          return (
            <React.Fragment key={target}>
              {idx > 0 && <span>/</span>}
              <button
                onClick={() => loadDir(target)}
                className="hover:underline cursor-pointer px-1 rounded"
                style={{ color: idx === breadcrumbs.length - 1 ? palette.textPrimary : palette.textSecondary }}
              >
                {seg === '/' ? 'root (/)' : seg}
              </button>
            </React.Fragment>
          );
        })}
      </div>

      {/* Body */}
      <div className="flex flex-1 min-h-0">
        {/* Sidebar */}
        <div
          className="w-36 shrink-0 border-r p-2 flex flex-col gap-1"
          style={{ backgroundColor: palette.surfaceSidebar, borderColor: palette.surfaceBorder }}
        >
          <div className="text-[10px] uppercase tracking-wider px-2 py-1" style={{ color: palette.textMuted }}>
            Mounts &amp; Places
          </div>
          {[
            { label: '/ (RAMFS)', path: '/' },
            { label: '/home', path: '/home' },
            { label: '/wallpapers', path: '/home/wallpapers' },
            { label: '/disk (FAT32)', path: '/disk' },
          ].map((item) => (
            <button
              key={item.path}
              onClick={() => loadDir(item.path)}
              className="flex items-center gap-2 px-2 py-1.5 rounded text-left cursor-pointer transition-colors"
              style={{
                backgroundColor: currentPath === item.path ? palette.surfaceSelected : 'transparent',
                color: currentPath === item.path ? palette.textPrimary : palette.textSecondary,
              }}
            >
              <HardDrive size={13} />
              <span className="truncate">{item.label}</span>
            </button>
          ))}
        </div>

        {/* File list */}
        <div className="flex-1 overflow-y-auto p-2">
          {viewMode === 'list' ? (
            <div className="flex flex-col gap-0.5">
              {entries.map((entry) => {
                const isSel = selected?.path === entry.path;
                return (
                  <div
                    key={entry.path}
                    onClick={() => setSelected(entry)}
                    onDoubleClick={() => handleOpen(entry)}
                    className="flex items-center justify-between px-2.5 py-1.5 rounded cursor-pointer"
                    style={{
                      backgroundColor: isSel ? palette.surfaceSelected : 'transparent',
                    }}
                  >
                    <div className="flex items-center gap-2 min-w-0">
                      {renderIcon(entry)}
                      <span className="truncate font-medium">{entry.name}</span>
                    </div>
                    <div className="flex items-center gap-3 shrink-0 text-[11px]" style={{ color: palette.textMuted }}>
                      <span>{entry.fsLabel}</span>
                      <span className="w-16 text-right">
                        {entry.kind === 'directory' ? `${entry.size} items` : `${entry.size} B`}
                      </span>
                    </div>
                  </div>
                );
              })}
            </div>
          ) : (
            <div className="grid grid-cols-4 gap-2">
              {entries.map((entry) => {
                const isSel = selected?.path === entry.path;
                return (
                  <div
                    key={entry.path}
                    onClick={() => setSelected(entry)}
                    onDoubleClick={() => handleOpen(entry)}
                    className="flex flex-col items-center justify-center p-2.5 rounded-lg cursor-pointer text-center gap-1.5 border"
                    style={{
                      backgroundColor: isSel ? palette.surfaceSelected : palette.surfaceCard,
                      borderColor: isSel ? palette.accentPrimary : 'transparent',
                    }}
                  >
                    {renderIcon(entry, 24)}
                    <span className="truncate w-full text-[11px]">{entry.name}</span>
                  </div>
                );
              })}
            </div>
          )}
        </div>

        {/* Inspector */}
        {selected && (
          <div
            className="w-52 shrink-0 border-l p-3 flex flex-col gap-2 overflow-y-auto"
            style={{ backgroundColor: palette.surfacePanel, borderColor: palette.surfaceBorder }}
          >
            <div className="flex items-center gap-2 font-semibold">
              {renderIcon(selected, 18)}
              <span className="truncate">{selected.name}</span>
            </div>
            <div className="text-[11px] space-y-1" style={{ color: palette.textSecondary }}>
              <div>Path: {selected.path}</div>
              <div>FS: {selected.fsLabel}</div>
              <div>Size: {selected.kind === 'directory' ? `${selected.size} items` : `${selected.size} bytes`}</div>
            </div>
            <button
              onClick={() => handleOpen(selected)}
              className="mt-1 px-2.5 py-1.5 rounded font-medium cursor-pointer"
              style={{ backgroundColor: palette.accentPrimary, color: palette.onAccent }}
            >
              {selected.name.endsWith('.elf')
                ? 'Execute Ring 3 ELF'
                : selected.name.endsWith('.wav')
                  ? 'Play in Audio Player'
                  : selected.kind === 'directory'
                    ? 'Open Folder'
                    : 'Open in Editor'}
            </button>
            {previewText && (
              <pre
                className="mt-1 p-2 rounded border text-[10px] whitespace-pre-wrap break-all amaryllis-mono"
                style={{ backgroundColor: palette.surfaceCard, borderColor: palette.surfaceBorder, color: palette.textSecondary }}
              >
                {previewText}
              </pre>
            )}
          </div>
        )}
      </div>

      {/* Status */}
      <div
        className="px-3 py-1 border-t text-[11px] flex justify-between"
        style={{ backgroundColor: palette.surfaceToolbar, borderColor: palette.surfaceBorder, color: palette.textMuted }}
      >
        <span>{statusMsg}</span>
        <span>{entries.length} entries</span>
      </div>
    </div>
  );
};
