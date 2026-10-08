"use client";
/** Client islands: live refresh, countdowns, the wallet button, wallet lookups. */
import React, { useEffect, useState, useTransition } from "react";
import { usePathname, useRouter, useSearchParams } from "next/navigation";
import { useWallet } from "@/lib/wallet";

/** Re-render the server component every `seconds` (chain state moves). */
export function AutoRefresh({ seconds }: { seconds: number }) {
  const router = useRouter();
  const [, start] = useTransition();
  useEffect(() => {
    const t = setInterval(() => start(() => router.refresh()), seconds * 1000);
    return () => clearInterval(t);
  }, [router, seconds]);
  return null;
}

export function Countdown({ to, done = "elapsed" }: { to: number; done?: string }) {
  const [now, setNow] = useState<number | null>(null);
  useEffect(() => {
    const tick = () => setNow(Math.floor(Date.now() / 1000));
    tick();
    const t = setInterval(tick, 1000);
    return () => clearInterval(t);
  }, []);
  if (now === null) return <span className="muted">…</span>;
  const s = to - now;
  if (s <= 0) return <span className="muted">{done}</span>;
  const d = Math.floor(s / 86400);
  const h = Math.floor((s % 86400) / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  return (
    <span>
      {d > 0 ? `${d}d ` : ""}
      {String(h).padStart(2, "0")}:{String(m).padStart(2, "0")}:{String(sec).padStart(2, "0")}
    </span>
  );
}

export function WalletButton() {
  const { wallets, wallet, account, connect, disconnect } = useWallet();
  const [err, setErr] = useState<string | null>(null);
  const [open, setOpen] = useState(false);
  if (account) {
    return (
      <span className="small">
        {wallet?.name}: {account.slice(0, 4)}…{account.slice(-4)}{" "}
        <button className="ghost" onClick={() => void disconnect()}>
          disconnect
        </button>
      </span>
    );
  }
  return (
    <span className="small" style={{ position: "relative" }}>
      <button onClick={() => setOpen((o) => !o)}>connect wallet</button>
      {open && (
        <span className="panel" style={{ position: "absolute", right: 0, top: 36, zIndex: 10, minWidth: 220, display: "flex", flexDirection: "column", gap: 6 }}>
          {wallets.length === 0 && <span className="muted">No Wallet Standard wallet found (Phantom, Backpack, Squads).</span>}
          {wallets.map((w) => (
            <button
              key={w.name}
              className="ghost"
              onClick={() =>
                connect(w)
                  .then(() => setOpen(false))
                  .catch((e: Error) => setErr(e.message))
              }
            >
              {w.name}
            </button>
          ))}
          {err && <span className="bad">{err}</span>}
        </span>
      )}
    </span>
  );
}

/** A wallet lookup box; defaults to the connected wallet. */
export function WalletLookup({ current }: { current: string | null }) {
  const { account } = useWallet();
  const router = useRouter();
  const path = usePathname();
  const params = useSearchParams();
  const [v, setV] = useState(current ?? "");
  useEffect(() => {
    if (!current && account) router.replace(`${path}?wallet=${account}`);
  }, [account, current, path, router]);
  return (
    <form
      className="row"
      onSubmit={(e) => {
        e.preventDefault();
        const q = new URLSearchParams(params.toString());
        q.set("wallet", v.trim());
        router.push(`${path}?${q.toString()}`);
      }}
    >
      <label>
        wallet
        <input value={v} onChange={(e) => setV(e.target.value)} size={46} placeholder="base58 address" />
      </label>
      <button type="submit">look up</button>
      {account && account !== current && (
        <button type="button" className="ghost" onClick={() => router.push(`${path}?wallet=${account}`)}>
          use connected wallet
        </button>
      )}
    </form>
  );
}
