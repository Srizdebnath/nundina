/**
 * Uniform-price clearing as two step curves: cumulative demand (bids, best
 * first) and supply (asks, cheapest first), with the clearing price, the
 * reserve floor and the NAV the reserve was set from. The curves draw in on
 * load (the demo's "clearing animation").
 */
import React from "react";
import type { CurvePoint } from "@/lib/session";
import { fmt6 } from "@/lib/session";

const W = 680;
const H = 300;
const PAD = { l: 64, r: 16, t: 14, b: 34 };

function stepPath(points: CurvePoint[], x: (q: bigint) => number, y: (p: bigint) => number, start: "top" | "bottom"): string {
  if (points.length === 0) return "";
  let d = `M ${x(0n)} ${y(points[0]!.price)}`;
  let prevQ = 0n;
  for (const p of points) {
    d += ` L ${x(prevQ)} ${y(p.price)} L ${x(p.qty)} ${y(p.price)}`;
    prevQ = p.qty;
  }
  // Close the step to the axis so the last tread reads as a vertical drop/rise.
  d += ` L ${x(prevQ)} ${start === "top" ? H - PAD.b : PAD.t}`;
  return d;
}

export function ClearingChart({
  demand,
  supply,
  clearPrice,
  clearedVolume,
  reserve,
  nav,
}: {
  demand: CurvePoint[];
  supply: CurvePoint[];
  clearPrice: bigint;
  clearedVolume: bigint;
  reserve: bigint;
  nav: bigint | null;
}) {
  if (demand.length === 0 && supply.length === 0) {
    return <p className="muted small">No revealed orders yet: the book is sealed until the reveal phase.</p>;
  }
  const prices = [...demand, ...supply].map((p) => p.price).concat([reserve, clearPrice, nav ?? 0n].filter((v) => v > 0n));
  let lo = prices.reduce((a, b) => (b < a ? b : a));
  let hi = prices.reduce((a, b) => (b > a ? b : a));
  const span = hi - lo || 100_000n;
  lo -= span / 5n;
  hi += span / 5n;
  const maxQ = [...demand, ...supply].reduce((m, p) => (p.qty > m ? p.qty : m), 1n);
  const x = (q: bigint) => PAD.l + (Number(q) / Number(maxQ)) * (W - PAD.l - PAD.r);
  const y = (p: bigint) => PAD.t + (1 - Number(p - lo) / Number(hi - lo)) * (H - PAD.t - PAD.b);
  const hline = (p: bigint, cls: string, label: string, dash?: string) => (
    <g>
      <line x1={PAD.l} x2={W - PAD.r} y1={y(p)} y2={y(p)} className={cls} strokeDasharray={dash} />
      <text x={W - PAD.r - 4} y={y(p) - 4} textAnchor="end" className={`${cls}-t`} fontSize={11}>
        {label} {fmt6(p, 4)}
      </text>
    </g>
  );
  return (
    <svg viewBox={`0 0 ${W} ${H}`} width="100%" role="img" aria-label="clearing curves">
      <style>{`
        .ax { stroke: #2a3550; }
        .dem { stroke: #5b87c7; fill: none; stroke-width: 2; }
        .sup { stroke: #f0b429; fill: none; stroke-width: 2; }
        .clr { stroke: #3fb68b; stroke-width: 1.5; } .clr-t { fill: #3fb68b; }
        .res { stroke: #e5534b; stroke-width: 1; } .res-t { fill: #e5534b; }
        .nav { stroke: #8b95a7; stroke-width: 1; } .nav-t { fill: #8b95a7; }
        .draw { stroke-dasharray: 2000; stroke-dashoffset: 2000; animation: draw 1.6s ease-out forwards; }
        .pop { opacity: 0; animation: pop 0.4s ease-out 1.5s forwards; }
        @keyframes draw { to { stroke-dashoffset: 0; } }
        @keyframes pop { to { opacity: 1; } }
      `}</style>
      <line x1={PAD.l} x2={PAD.l} y1={PAD.t} y2={H - PAD.b} className="ax" />
      <line x1={PAD.l} x2={W - PAD.r} y1={H - PAD.b} y2={H - PAD.b} className="ax" />
      <text x={PAD.l} y={H - 10} fontSize={11} fill="#8b95a7">
        0
      </text>
      <text x={W - PAD.r} y={H - 10} fontSize={11} fill="#8b95a7" textAnchor="end">
        {fmt6(maxQ, 2)} units
      </text>
      <text x={8} y={PAD.t + 10} fontSize={11} fill="#8b95a7">
        USDC/unit
      </text>
      {nav !== null && nav > 0n && hline(nav, "nav", "NAV", "4 4")}
      {reserve > 0n && hline(reserve, "res", "reserve", "2 3")}
      <path d={stepPath(demand, x, y, "top")} className="dem draw" />
      <path d={stepPath(supply, x, y, "bottom")} className="sup draw" />
      {clearPrice > 0n && (
        <g className="pop">
          {hline(clearPrice, "clr", "clear")}
          <circle cx={x(clearedVolume)} cy={y(clearPrice)} r={5} fill="#3fb68b" />
        </g>
      )}
      <g fontSize={11}>
        <text x={PAD.l + 8} y={H - PAD.b - 22} fill="#5b87c7">
          ― demand (bids)
        </text>
        <text x={PAD.l + 8} y={H - PAD.b - 8} fill="#f0b429">
          ― supply (asks)
        </text>
      </g>
    </svg>
  );
}
