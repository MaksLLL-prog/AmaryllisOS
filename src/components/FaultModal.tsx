import React from 'react';
import { ShieldAlert, X } from 'lucide-react';
import { anuKernel, SystemAlert } from '../os/kernel';
import { ThemePalette } from '../os/theme';

interface FaultModalProps {
  alert: SystemAlert;
  palette: ThemePalette;
  onDismiss: () => void;
}

export const FaultModal: React.FC<FaultModalProps> = ({ alert, palette, onDismiss }) => {
  const t = alert.telemetry;

  return (
    <div className="fixed inset-0 z-[9999] flex items-center justify-center bg-black/45 backdrop-blur-[2px] p-4">
      <div
        className="w-full max-w-[520px] rounded-xl border shadow-2xl overflow-hidden"
        style={{
          backgroundColor: palette.modalBg,
          borderColor: palette.modalBorder,
          color: palette.textPrimary,
        }}
      >
        <div
          className="px-4 py-3 border-b flex items-center justify-between"
          style={{ borderColor: palette.surfaceBorder }}
        >
          <div className="flex items-center gap-2.5">
            <span className="p-1.5 rounded-lg bg-[#DE4B5A]/15 text-[#FF4D6D]">
              <ShieldAlert size={18} />
            </span>
            <div>
              <div className="text-[13px] font-semibold">{alert.title}</div>
              <div className="text-[11px]" style={{ color: palette.textMuted }}>
                {alert.subsystem} • {alert.codeTag} • {alert.timestamp}
              </div>
            </div>
          </div>
          <button
            onClick={onDismiss}
            className="p-1 rounded hover:bg-white/10 cursor-pointer"
            style={{ color: palette.textSecondary }}
          >
            <X size={16} />
          </button>
        </div>

        <div className="p-4 space-y-3 text-[12px]">
          <p className="leading-relaxed" style={{ color: palette.textPrimary }}>
            {alert.summary}
          </p>

          {t && (
            <div
              className="p-3 rounded-lg border amaryllis-mono text-[11px] grid grid-cols-2 gap-x-4 gap-y-1.5"
              style={{
                backgroundColor: palette.surfaceCard,
                borderColor: palette.surfaceBorder,
                color: palette.textSecondary,
              }}
            >
              <div>PID: <span style={{ color: palette.textPrimary }}>{t.pid}</span></div>
              <div>Vector: <span className="text-[#FF4D6D]">{t.vectorName}</span></div>
              <div>RIP: <span style={{ color: palette.textPrimary }}>{t.rip}</span></div>
              <div>CR2: <span className="text-[#FF4D6D]">{t.cr2}</span></div>
              <div>RSP: <span style={{ color: palette.textPrimary }}>{t.rsp}</span></div>
              <div>PMM Integrity: <span className="text-[#24A87E]">{t.pmmOk ? 'OK (frames freed)' : 'ERR'}</span></div>
              <div className="col-span-2">Error Code: <span style={{ color: palette.textPrimary }}>{t.errorCode}</span></div>
            </div>
          )}

          <div
            className="p-2.5 rounded border text-[11.5px]"
            style={{
              backgroundColor: palette.surfacePanel,
              borderColor: palette.surfaceBorder,
              color: palette.textSecondary,
            }}
          >
            <span className="font-semibold" style={{ color: palette.accentMint }}>Recovery Hint: </span>
            {alert.hint}
          </div>
        </div>

        <div
          className="px-4 py-2.5 border-t flex justify-end gap-2"
          style={{ borderColor: palette.surfaceBorder }}
        >
          <button
            onClick={() => {
              anuKernel.dismissModalAlert();
              onDismiss();
            }}
            className="px-4 py-1.5 rounded-lg font-medium text-[12px] cursor-pointer"
            style={{ backgroundColor: palette.accentPrimary, color: palette.onAccent }}
          >
            Acknowledge &amp; Recover
          </button>
        </div>
      </div>
    </div>
  );
};
