import React, { useState } from 'react';
import { Globe, RefreshCw, Wifi } from 'lucide-react';
import { anuKernel } from '../../os/kernel';
import { ThemePalette } from '../../os/theme';

interface NetViewerAppProps {
  palette: ThemePalette;
}

function stripHtml(html: string): string {
  return html
    .replace(/<script[\s\S]*?<\/script>/gi, '')
    .replace(/<style[\s\S]*?<\/style>/gi, '')
    .replace(/<[^>]+>/g, ' ')
    .replace(/\s+/g, ' ')
    .trim();
}

export const NetViewerApp: React.FC<NetViewerAppProps> = ({ palette }) => {
  const [url, setUrl] = useState('https://example.com/');
  const [rawMode, setRawMode] = useState(false);
  const [loading, setLoading] = useState(false);
  const [statusLine, setStatusLine] = useState(
    `e1000 Link UP — IP ${anuKernel.getNetState().ip} (press Go, DHCP, or Ping GW)`
  );
  const [headers, setHeaders] = useState<Array<[string, string]>>([]);
  const [body, setBody] = useState(
    `Amaryllis Net / HTTP Viewer\n------------------------------------------------\n1. Edit URL above and press Enter or click Go.\n2. Click DHCP to acquire IPv4 lease via Intel 82540EM (e1000).\n3. Click Ping GW to test ICMP Echo latency.\n4. Toggle Text / Raw to inspect HTTP headers and HTML source.`
  );

  const fetchUrl = async (target = url) => {
    setLoading(true);
    setStatusLine(`knet_worker: fetching ${target}...`);
    try {
      const res = await fetch(`/api/net/httpget?url=${encodeURIComponent(target)}`);
      const data = (await res.json()) as {
        ok: boolean;
        status?: number;
        statusText?: string;
        headers?: Record<string, string>;
        body?: string;
        bytes?: number;
        elapsedMs?: number;
        error?: string;
      };
      anuKernel.bumpNetTraffic(6, 10);
      if (data.status) {
        setStatusLine(`HTTP ${data.status} ${data.statusText ?? ''} • ${data.bytes ?? 0} B • ${data.elapsedMs ?? 0} ms`);
        setHeaders(Object.entries(data.headers ?? {}));
        setBody(data.body ?? '');
      } else {
        setStatusLine(`HTTP Error: ${data.error ?? 'Failed'}`);
      }
    } catch {
      setStatusLine('Network fetch failed.');
    } finally {
      setLoading(false);
    }
  };

  const handleDhcp = () => {
    const lease = anuKernel.acquireDhcp();
    setStatusLine(`DHCP OK: IP ${lease.ip}  GW ${lease.gateway}  DNS ${lease.dns}`);
  };

  const handlePing = async () => {
    const net = anuKernel.getNetState();
    setLoading(true);
    setStatusLine(`Pinging ${net.gateway} & checking DNS...`);
    try {
      const res = await fetch('/api/net/dns?host=example.com');
      const data = (await res.json()) as { elapsedMs?: number };
      anuKernel.bumpNetTraffic(4, 4);
      setStatusLine(`ICMP Echo reply from ${net.gateway}: time=1ms (DNS RTT ${data.elapsedMs ?? 8}ms)`);
    } catch {
      setStatusLine(`ICMP Echo reply from ${net.gateway}: time=1ms`);
    } finally {
      setLoading(false);
    }
  };

  return (
    <div
      className="flex flex-col h-full w-full text-[12px]"
      style={{ backgroundColor: palette.winBgActive, color: palette.textPrimary }}
    >
      {/* Address Bar */}
      <div
        className="flex items-center gap-1.5 px-3 py-2 border-b"
        style={{ backgroundColor: palette.surfaceToolbar, borderColor: palette.surfaceBorder }}
      >
        <Globe size={15} style={{ color: palette.accentCyan }} />
        <input
          value={url}
          onChange={(e) => setUrl(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') void fetchUrl();
          }}
          className="flex-1 px-2.5 py-1 rounded border outline-none amaryllis-mono text-[12px]"
          style={{
            backgroundColor: palette.modalInputBg,
            borderColor: palette.surfaceBorder,
            color: palette.textPrimary,
          }}
        />
        <button
          onClick={() => void fetchUrl()}
          disabled={loading}
          className="px-3 py-1 rounded font-medium cursor-pointer flex items-center gap-1"
          style={{ backgroundColor: palette.accentPrimary, color: palette.onAccent }}
        >
          {loading ? <RefreshCw size={12} className="animate-spin" /> : null} Go
        </button>
        <button
          onClick={handleDhcp}
          className="px-2.5 py-1 rounded border cursor-pointer"
          style={{ backgroundColor: palette.surfaceCard, borderColor: palette.surfaceBorder }}
        >
          DHCP
        </button>
        <button
          onClick={() => void handlePing()}
          className="px-2.5 py-1 rounded border cursor-pointer"
          style={{ backgroundColor: palette.surfaceCard, borderColor: palette.surfaceBorder }}
        >
          Ping GW
        </button>
        <button
          onClick={() => setRawMode((r) => !r)}
          className="px-2.5 py-1 rounded border cursor-pointer"
          style={{
            backgroundColor: rawMode ? palette.surfaceSelected : palette.surfaceCard,
            borderColor: palette.surfaceBorder,
          }}
        >
          {rawMode ? 'Raw' : 'Text'}
        </button>
      </div>

      {/* Content */}
      <div className="flex-1 overflow-y-auto p-3 space-y-3">
        {rawMode && headers.length > 0 && (
          <div
            className="p-2.5 rounded border amaryllis-mono text-[11px]"
            style={{ backgroundColor: palette.surfaceCard, borderColor: palette.surfaceBorder }}
          >
            <div className="font-semibold mb-1" style={{ color: palette.accentCyan }}>
              Response Headers
            </div>
            {headers.map(([k, v]) => (
              <div key={k} className="truncate">
                <span style={{ color: palette.textMuted }}>{k}:</span> {v}
              </div>
            ))}
          </div>
        )}
        <pre className="whitespace-pre-wrap break-words amaryllis-mono text-[12px] leading-relaxed select-text">
          {rawMode ? body : stripHtml(body)}
        </pre>
      </div>

      {/* Status */}
      <div
        className="px-3 py-1 border-t text-[11px] flex items-center justify-between"
        style={{ backgroundColor: palette.surfaceToolbar, borderColor: palette.surfaceBorder, color: palette.textMuted }}
      >
        <span className="flex items-center gap-1.5">
          <Wifi size={12} style={{ color: palette.accentMint }} />
          {statusLine}
        </span>
        <span>knet_worker async</span>
      </div>
    </div>
  );
};
