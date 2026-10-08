/** Small server-safe display helpers shared by the pages. */
import React from "react";
import { explorer } from "@/lib/programs";

export function Addr({ a, kind = "address" }: { a: string; kind?: "address" | "tx" }) {
  const short = a.length > 16 ? `${a.slice(0, 4)}…${a.slice(-4)}` : a;
  return (
    <a href={explorer(kind, a)} target="_blank" rel="noreferrer" title={a}>
      {short}
    </a>
  );
}

export function Stat({ k, v, cls }: { k: string; v: React.ReactNode; cls?: string }) {
  return (
    <div className="panel stat">
      <div className="k">{k}</div>
      <div className={`v ${cls ?? ""}`}>{v}</div>
    </div>
  );
}

export function utc(ts: number): string {
  if (!ts) return "—";
  return new Date(ts * 1000).toISOString().replace("T", " ").slice(0, 19) + " UTC";
}

export function Banner() {
  return <p className="label">PILOT-DEMO-ASSET — NOT AN OFFER — SYNTHETIC LABEL</p>;
}

export function ErrorPanel({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div className="verdict bad">
      <div className="head">{title}</div>
      <div className="small" style={{ marginTop: 6 }}>
        {children}
      </div>
    </div>
  );
}
