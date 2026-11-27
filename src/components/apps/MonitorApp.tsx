import React, { useEffect, useState } from 'react';
import { AlertTriangle, Cpu, HardDrive, ShieldAlert } from 'lucide-react';
import { anuKernel, SystemAlert } from '../../os/kernel';
import { ThemePalette } from '../../os/theme';

interface MonitorAppProps {
  palette: ThemePalette;
  initialTab?: 'overview' | 'tasks' | 'drivers' | 'alerts';
}

export const MonitorApp: React.FC<MonitorAppProps> = ({ palette, initialTab = 'overview' }) => {
  const [tab, setTab] = useState<'overview' | 'tasks' | 'drivers' | 'alerts'>(initialTab);
  const [procs, setProcs] = useState(anuKernel.getProcesses());
  const [kthreads, setKthreads] = useState(anuKernel.getKThreads());
  const [alerts, setAlerts] = useState<SystemAlert[]>(anuKernel.getAlerts());
  const [samples, setSamples] = useState<number[]>(() => Array.from({ length: 32 }, (_, i) => 18 + (i % 5) * 2));
  const drivers = anuKernel.getDrivers();

  useEffect(() => {
    const sync = () => {
      setProcs(anuKernel.getProcesses());
      setKthreads(anuKernel.getKThreads());
      setAlerts(anuKernel.getAlerts());
    };
    const unsub = anuKernel.subscribe(sync);
    const id = window.setInterval(() => {
      const procLoad = anuKernel.getProcesses().length * 4;
      setSamples((prev) => [...prev.slice(1), Math.min(92, 18 + procLoad + Math.round(Math.random() * 6))]);
    }, 600);
    return () => {
      unsub();
      window.clearInterval(id);
    };
  }, []);

  return (
    <div
      className="flex flex-col h-full w-full text-[12px]"
      style={{ backgroundColor: palette.winBgActive, color: palette.textPrimary }}
    >
      {/* Tabs */}
      <div
        className="flex items-center justify-between px-3 py-2 border-b"
        style={{ backgroundColor: palette.surfaceToolbar, borderColor: palette.surfaceBorder }}
      >
        <div className="flex items-center gap-1.5">
          {[
            ['overview', 'Overview'],
            ['tasks', `Tasks (${procs.length}+${kthreads.length})`],
            ['drivers', `Drivers (${drivers.length})`],
            ['alerts', `Alerts (${alerts.length})`],
          ].map(([id, label]) => (
            <button
              key={id}
              onClick={() => setTab(id as typeof tab)}
              className="px-3 py-1 rounded border font-medium cursor-pointer"
              style={{
                backgroundColor: tab === id ? palette.surfaceSelected : palette.surfaceCard,
                borderColor: tab === id ? palette.accentPrimary : palette.surfaceBorder,
              }}
            >
              {label}
            </button>
          ))}
        </div>
        {tab === 'alerts' && alerts.length > 0 && (
          <button
            onClick={() => anuKernel.openLatestAlert()}
            className="px-2.5 py-1 rounded font-medium cursor-pointer flex items-center gap-1"
            style={{ backgroundColor: palette.accentErr, color: '#FFFFFF' }}
          >
            <ShieldAlert size={13} /> Inspect Latest
          </button>
        )}
      </div>

      {/* Body */}
      <div className="flex-1 overflow-y-auto p-3.5 space-y-3">
        {tab === 'overview' && (
          <>
            <div className="grid grid-cols-3 gap-2.5">
              <div className="p-3 rounded border" style={{ backgroundColor: palette.surfaceCard, borderColor: palette.surfaceBorder }}>
                <div className="text-[11px]" style={{ color: palette.textMuted }}>Kernel Heap (64 MiB)</div>
                <div className="text-[18px] font-semibold mt-0.5">{samples[samples.length - 1]}%</div>
                <div className="text-[11px]" style={{ color: palette.accentMint }}>PMM: 1024 MiB OK</div>
              </div>
              <div className="p-3 rounded border" style={{ backgroundColor: palette.surfaceCard, borderColor: palette.surfaceBorder }}>
                <div className="text-[11px]" style={{ color: palette.textMuted }}>Compositor Pacing</div>
                <div className="text-[18px] font-semibold mt-0.5">60 FPS</div>
                <div className="text-[11px]" style={{ color: palette.textSecondary }}>Dirty-rect active</div>
              </div>
              <div className="p-3 rounded border" style={{ backgroundColor: palette.surfaceCard, borderColor: palette.surfaceBorder }}>
                <div className="text-[11px]" style={{ color: palette.textMuted }}>Unified Page Cache</div>
                <div className="text-[18px] font-semibold mt-0.5">98.2%</div>
                <div className="text-[11px]" style={{ color: palette.textSecondary }}>4 KiB Clock eviction</div>
              </div>
            </div>

            <div className="p-3 rounded border" style={{ backgroundColor: palette.surfaceCard, borderColor: palette.surfaceBorder }}>
              <div className="flex justify-between mb-2">
                <span className="font-semibold">Live Memory &amp; Scheduler Utilization</span>
                <span className="amaryllis-mono text-[11px]" style={{ color: palette.textMuted }}>
                  Uptime: {anuKernel.uptimeSeconds()}s
                </span>
              </div>
              <div className="h-28 flex items-end gap-1 pt-2">
                {samples.map((val, idx) => (
                  <div
                    key={idx}
                    className="flex-1 rounded-t"
                    style={{ height: `${val}%`, backgroundColor: palette.accentMint, opacity: 0.85 }}
                  />
                ))}
              </div>
            </div>
          </>
        )}

        {tab === 'tasks' && (
          <div className="space-y-4">
            <div>
              <div className="font-semibold mb-1.5 flex items-center gap-1.5">
                <Cpu size={14} /> Ring 3 Userland Processes
              </div>
              {procs.length === 0 ? (
                <div className="p-3 rounded border text-[11.5px]" style={{ backgroundColor: palette.surfaceCard, borderColor: palette.surfaceBorder, color: palette.textSecondary }}>
                  No active Ring 3 processes. Launch <code>/counter.elf</code> or <code>/gui_demo.elf</code> from Files or Terminal.
                </div>
              ) : (
                <div className="space-y-1.5">
                  {procs.map((p) => (
                    <div
                      key={p.pid}
                      className="flex items-center justify-between px-3 py-2 rounded border"
                      style={{ backgroundColor: palette.surfaceCard, borderColor: palette.surfaceBorder }}
                    >
                      <div>
                        <div className="font-medium">
                          PID {p.pid} — {p.path}
                        </div>
                        <div className="text-[11px] amaryllis-mono" style={{ color: palette.textSecondary }}>
                          {p.state} • CR3 {p.cr3} • vruntime {p.vruntimeMs}ms • syscalls {p.syscalls}
                          {p.kind === 'counter' ? ` • count=${p.counterVal}` : ''}
                        </div>
                      </div>
                      <button
                        onClick={() => anuKernel.killProcess(p.pid)}
                        className="px-2.5 py-1 rounded text-[11px] font-medium cursor-pointer"
                        style={{ backgroundColor: palette.accentErr, color: '#FFFFFF' }}
                      >
                        Kill
                      </button>
                    </div>
                  ))}
                </div>
              )}
            </div>

            <div>
              <div className="font-semibold mb-1.5">Cooperative Kernel Threads (kthread)</div>
              <div className="space-y-1">
                {kthreads.map((kt) => (
                  <div
                    key={kt.tid}
                    className="flex items-center justify-between px-3 py-1.5 rounded border amaryllis-mono text-[11px]"
                    style={{ backgroundColor: palette.surfaceCard, borderColor: palette.surfaceBorder }}
                  >
                    <span>
                      TID {kt.tid} • {kt.name}
                    </span>
                    <span style={{ color: palette.textSecondary }}>
                      CPU#{kt.cpuCore} • {kt.state} • {kt.stackSizeKb}KiB • ticks={kt.ticks}
                    </span>
                  </div>
                ))}
              </div>
            </div>
          </div>
        )}

        {tab === 'drivers' && (
          <div className="space-y-1.5">
            {drivers.map((d) => (
              <div
                key={d.name}
                className="flex items-center justify-between px-3 py-2 rounded border"
                style={{ backgroundColor: palette.surfaceCard, borderColor: palette.surfaceBorder }}
              >
                <div className="flex items-center gap-2.5">
                  <HardDrive size={14} style={{ color: palette.accentMint }} />
                  <div>
                    <div className="font-medium">
                      {d.name} <span className="text-[11px]" style={{ color: palette.textMuted }}>({d.deviceClass})</span>
                    </div>
                    <div className="text-[11px]" style={{ color: palette.textSecondary }}>
                      {d.bus} • {d.details}
                    </div>
                  </div>
                </div>
                <span className="px-2 py-0.5 rounded text-[10px] font-semibold" style={{ backgroundColor: palette.surfaceSelected, color: palette.accentMint }}>
                  {d.status}
                </span>
              </div>
            ))}
          </div>
        )}

        {tab === 'alerts' && (
          <div className="space-y-2">
            {alerts.length === 0 ? (
              <div className="p-4 rounded border text-center" style={{ backgroundColor: palette.surfaceCard, borderColor: palette.surfaceBorder, color: palette.textSecondary }}>
                No system faults recorded. Run <code>/fault.elf</code> to test Ring 3 #PF isolation and the Fault Inspector.
              </div>
            ) : (
              alerts
                .slice()
                .reverse()
                .map((al) => (
                  <div
                    key={al.id}
                    className="p-3 rounded border space-y-1"
                    style={{ backgroundColor: palette.surfaceCard, borderColor: palette.surfaceBorder }}
                  >
                    <div className="flex items-center justify-between">
                      <span className="font-semibold flex items-center gap-1.5" style={{ color: palette.accentErr }}>
                        <AlertTriangle size={14} /> {al.title}
                      </span>
                      <span className="text-[11px]" style={{ color: palette.textMuted }}>
                        {al.timestamp} • {al.codeTag}
                      </span>
                    </div>
                    <div className="text-[11.5px]" style={{ color: palette.textSecondary }}>
                      {al.summary}
                    </div>
                  </div>
                ))
            )}
          </div>
        )}
      </div>
    </div>
  );
};
