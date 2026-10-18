// SPDX-License-Identifier: AGPL-3.0-only
import { useRef } from "react";
import { save as saveDialog } from "@tauri-apps/plugin-dialog";
import { api, errorText, type ChartSpec } from "../api";
import { t } from "../i18n";
import type { PushToast } from "./Toasts";

const W = 640;
const H = 340;
const PAD = { left: 64, right: 20, top: 40, bottom: 56 };
const COLORS = ["#6366f1", "#ec4899", "#10b981", "#f59e0b", "#0ea5e9", "#ef4444"];

/** Round step sizes for an axis from lo to hi. */
function ticks(lo: number, hi: number, count = 5): number[] {
  if (!isFinite(lo) || !isFinite(hi)) return [0];
  if (lo === hi) {
    lo -= 1;
    hi += 1;
  }
  const raw = (hi - lo) / count;
  const mag = Math.pow(10, Math.floor(Math.log10(raw)));
  const step = [1, 2, 2.5, 5, 10].map((m) => m * mag).find((s) => s >= raw) ?? raw;
  // From a step at or below the lowest value to one at or above the highest.
  const out: number[] = [];
  for (let v = Math.floor(lo / step) * step; out.length < 50; v += step) {
    out.push(Math.round(v * 1e9) / 1e9);
    if (v >= hi - step * 1e-9) break;
  }
  return out;
}

function short(v: number): string {
  const a = Math.abs(v);
  if (a >= 1e9) return `${+(v / 1e9).toFixed(1)}B`;
  if (a >= 1e6) return `${+(v / 1e6).toFixed(1)}M`;
  if (a >= 1e4) return `${+(v / 1e3).toFixed(1)}K`;
  return `${+v.toFixed(a < 1 ? 4 : 2)}`;
}

function Pie({ spec }: { spec: ChartSpec }) {
  const data = (spec.series[0]?.data ?? []).map((v) => Math.max(0, v ?? 0));
  const total = data.reduce((s, v) => s + v, 0) || 1;
  const cx = 200;
  const cy = H / 2 + 10;
  const r = 120;
  let angle = -Math.PI / 2;
  return (
    <>
      {data.map((v, i) => {
        const a = (v / total) * Math.PI * 2;
        const x1 = cx + r * Math.cos(angle);
        const y1 = cy + r * Math.sin(angle);
        angle += a;
        const x2 = cx + r * Math.cos(angle);
        const y2 = cy + r * Math.sin(angle);
        const d = data.length === 1 ? `M ${cx - r} ${cy} a ${r} ${r} 0 1 0 ${2 * r} 0 a ${r} ${r} 0 1 0 ${-2 * r} 0` : `M ${cx} ${cy} L ${x1} ${y1} A ${r} ${r} 0 ${a > Math.PI ? 1 : 0} 1 ${x2} ${y2} Z`;
        return <path key={i} d={d} fill={COLORS[i % COLORS.length]} stroke="var(--bg, #fff)" strokeWidth={1.5} />;
      })}
      {data.slice(0, 12).map((v, i) => (
        <g key={i} transform={`translate(360, ${60 + i * 22})`}>
          <rect width={12} height={12} rx={2} fill={COLORS[i % COLORS.length]} />
          <text x={18} y={11} className="chart-text">
            {(spec.labels[i] ?? `#${i + 1}`).slice(0, 28)} · {short(v)} ({Math.round((v / total) * 100)}%)
          </text>
        </g>
      ))}
    </>
  );
}

function Axes({ spec }: { spec: ChartSpec }) {
  const scatter = spec.type === "scatter";
  const all = scatter ? spec.series.flatMap((s) => (s.points ?? []).map((p) => p[1])) : spec.series.flatMap((s) => (s.data ?? []).filter((v): v is number => v !== null));
  const xs = scatter ? spec.series.flatMap((s) => (s.points ?? []).map((p) => p[0])) : [];
  let lo = Math.min(...all);
  let hi = Math.max(...all);
  if (spec.type === "bar" || spec.type === "area") lo = Math.min(0, lo);
  const yt = ticks(lo, hi);
  lo = Math.min(lo, yt[0]);
  hi = Math.max(hi, yt[yt.length - 1]);
  const pw = W - PAD.left - PAD.right;
  const ph = H - PAD.top - PAD.bottom;
  const y = (v: number) => PAD.top + ph - ((v - lo) / (hi - lo || 1)) * ph;
  const n = Math.max(1, ...spec.series.map((s) => s.data?.length ?? 0), spec.labels.length);
  const xlo = Math.min(...xs);
  const xhi = Math.max(...xs);
  const xt = scatter ? ticks(xlo, xhi) : [];
  const x = scatter ? (v: number) => PAD.left + ((v - xt[0]) / (xt[xt.length - 1] - xt[0] || 1)) * pw : (i: number) => PAD.left + (n === 1 ? pw / 2 : spec.type === "bar" ? ((i + 0.5) / n) * pw : (i / (n - 1)) * pw);
  // At most about 10 labels along the bottom.
  const every = Math.max(1, Math.ceil(n / 10));
  const bars = spec.series.length;
  const band = pw / n;
  const bw = Math.max(2, (band * 0.75) / bars);
  return (
    <>
      {yt.map((v) => (
        <g key={v}>
          <line x1={PAD.left} x2={W - PAD.right} y1={y(v)} y2={y(v)} className="chart-grid" />
          <text x={PAD.left - 8} y={y(v) + 4} textAnchor="end" className="chart-text">
            {short(v)}
          </text>
        </g>
      ))}
      {scatter
        ? xt.map((v) => (
            <text key={v} x={x(v)} y={H - PAD.bottom + 18} textAnchor="middle" className="chart-text">
              {short(v)}
            </text>
          ))
        : spec.labels.map((l, i) =>
            i % every === 0 ? (
              // The last label ends at its point, so it isn't cut off at the edge.
              <text key={i} x={x(i)} y={H - PAD.bottom + 18} textAnchor={spec.type !== "bar" && n > 1 && i >= n - every ? "end" : "middle"} className="chart-text">
                {l.length > 14 ? l.slice(0, 13) + "…" : l}
              </text>
            ) : null,
          )}
      {spec.series.map((s, si) => {
        const color = COLORS[si % COLORS.length];
        if (scatter) return (s.points ?? []).map((p, i) => <circle key={`${si}-${i}`} cx={x(p[0])} cy={y(p[1])} r={3.5} fill={color} opacity={0.8} />);
        const data = s.data ?? [];
        if (spec.type === "bar")
          return data.map((v, i) =>
            v === null ? null : <rect key={`${si}-${i}`} x={x(i) - (bw * bars) / 2 + si * bw} y={Math.min(y(v), y(0))} width={bw - 1} height={Math.abs(y(0) - y(v))} fill={color} rx={1.5} />,
          );
        const pts = data.map((v, i) => (v === null ? null : `${x(i)},${y(v)}`)).filter(Boolean) as string[];
        return (
          <g key={si}>
            {spec.type === "area" && pts.length > 1 && <polygon points={`${x(0)},${y(Math.max(lo, 0))} ${pts.join(" ")} ${x(data.length - 1)},${y(Math.max(lo, 0))}`} fill={color} opacity={0.18} />}
            <polyline points={pts.join(" ")} fill="none" stroke={color} strokeWidth={2} strokeLinejoin="round" />
            {data.length <= 40 && data.map((v, i) => (v === null ? null : <circle key={i} cx={x(i)} cy={y(v)} r={2.5} fill={color} />))}
          </g>
        );
      })}
      {spec.x_label && (
        <text x={PAD.left + pw / 2} y={H - 12} textAnchor="middle" className="chart-text">
          {spec.x_label}
        </text>
      )}
      {spec.y_label && (
        <text x={14} y={PAD.top + ph / 2} textAnchor="middle" transform={`rotate(-90 14 ${PAD.top + ph / 2})`} className="chart-text">
          {spec.y_label}
        </text>
      )}
    </>
  );
}

/** Draws a chart as an image in the chat, with a button to save it. */
export function ChartView({ spec, toast }: { spec: ChartSpec; toast?: PushToast }) {
  const ref = useRef<SVGSVGElement>(null);
  const named = spec.series.filter((s) => s.name).length > 1 && spec.type !== "pie";
  const save = async () => {
    const svg = ref.current;
    if (!svg) return;
    const path = await saveDialog({ defaultPath: `${(spec.title || "chart").replace(/[\\/:*?"<>|]+/g, "-").trim() || "chart"}.svg`, filters: [{ name: t("SVG image"), extensions: ["svg"] }] });
    if (!path) return;
    try {
      await api.saveChart(path, new XMLSerializer().serializeToString(svg));
      toast?.(t("Saved."), "success");
    } catch (e) {
      toast?.(errorText(e), "error");
    }
  };
  return (
    <figure className="chart-view">
      <svg ref={ref} viewBox={`0 0 ${W} ${H}`} role="img" aria-label={spec.title || t("Chart")} xmlns="http://www.w3.org/2000/svg">
        <style>{".chart-text{font:12px system-ui,sans-serif;fill:#6b7280}.chart-grid{stroke:#9ca3af;stroke-opacity:.25}.chart-title{font:600 14px system-ui,sans-serif;fill:#6b7280}"}</style>
        {spec.title && (
          <text x={W / 2} y={22} textAnchor="middle" className="chart-title">
            {spec.title}
          </text>
        )}
        {spec.type === "pie" ? <Pie spec={spec} /> : <Axes spec={spec} />}
        {named &&
          spec.series.map((s, i) => (
            <g key={i} transform={`translate(${PAD.left + i * 120}, ${H - 26})`}>
              <rect width={10} height={10} rx={2} fill={COLORS[i % COLORS.length]} />
              <text x={14} y={9} className="chart-text">
                {s.name.slice(0, 16)}
              </text>
            </g>
          ))}
      </svg>
      <figcaption>
        <button className="btn ghost small" onClick={save}>
          {t("Save chart")}
        </button>
      </figcaption>
    </figure>
  );
}
