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

/** Install pages for when no wallet is injected; Phantom is the primary wallet. */
const INSTALL = [
  { name: "Phantom", href: "https://phantom.com/download", primary: true },
  { name: "Solflare", href: "https://solflare.com/download" },
  { name: "Backpack", href: "https://backpack.app/download" },
];

/** Open this page inside Phantom's in-app browser (mobile, where no extension exists). */
function phantomBrowse(): string {
  const url = encodeURIComponent(window.location.href);
  return `https://phantom.app/ul/browse/${url}?ref=${encodeURIComponent(window.location.origin)}`;
}

export function WalletButton() {
  const { wallets, wallet, account, connect, disconnect } = useWallet();
  const [err, setErr] = useState<string | null>(null);
  const [open, setOpen] = useState(false);
  const [pending, setPending] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  // Only read inside the modal, which never renders on the server.
  const [mobile] = useState(() => typeof navigator !== "undefined" && /android|iphone|ipad|ipod/i.test(navigator.userAgent));
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open]);

  if (account) {
    return (
      <span className="wallet-chip">
        {/* Wallet icons are data: URIs from the extension; next/image cannot optimize them. */}
        {/* eslint-disable-next-line @next/next/no-img-element */}
        {wallet?.icon && <img src={wallet.icon} alt="" width={18} height={18} />}
        <button
          className="ghost mono"
          title={`${account} (click to copy)`}
          onClick={() =>
            void navigator.clipboard?.writeText(account).then(() => {
              setCopied(true);
              setTimeout(() => setCopied(false), 1200);
            })
          }
        >
          {copied ? "copied" : `${account.slice(0, 4)}…${account.slice(-4)}`}
        </button>
        <button className="ghost" onClick={() => void disconnect()} aria-label="Disconnect wallet">
          disconnect
        </button>
      </span>
    );
  }

  const pick = (w: (typeof wallets)[number]) => {
    setErr(null);
    setPending(w.name);
    connect(w)
      .then(() => setOpen(false))
      .catch((e: Error) => setErr(/reject|denied|cancel/i.test(e.message) ? "Request rejected in the wallet." : e.message))
      .finally(() => setPending(null));
  };
  const hasPhantom = wallets.some((w) => w.name === "Phantom");

  return (
    <>
      <button onClick={() => setOpen(true)}>Connect wallet</button>
      {open && (
        <div className="modal-backdrop" onClick={() => setOpen(false)}>
          <div className="modal" role="dialog" aria-modal="true" aria-labelledby="wallet-title" onClick={(e) => e.stopPropagation()}>
            <div className="modal-head">
              <h2 id="wallet-title">Connect a wallet</h2>
              <button className="ghost" onClick={() => setOpen(false)} aria-label="Close">
                ✕
              </button>
            </div>
            <p className="small muted">
              Solana {process.env.NEXT_PUBLIC_CLUSTER ?? "devnet"}. Your wallet signs every transaction; nothing is signed on a
              server.
            </p>
            <div className="wallet-list">
              {wallets.map((w) => (
                <button key={w.name} className="wallet-row" disabled={pending !== null} onClick={() => pick(w)}>
                  {/* eslint-disable-next-line @next/next/no-img-element */}
                  {w.icon ? <img src={w.icon} alt="" width={28} height={28} /> : <span className="wallet-icon" />}
                  <span className="wallet-name">{w.name}</span>
                  <span className="small muted">
                    {pending === w.name ? "approve in wallet…" : w.name === "Phantom" ? "recommended" : "detected"}
                  </span>
                </button>
              ))}
              {!hasPhantom &&
                (mobile ? (
                  <a className="wallet-row" href={phantomBrowse()}>
                    <span className="wallet-icon">P</span>
                    <span className="wallet-name">Open in Phantom</span>
                    <span className="small muted">recommended</span>
                  </a>
                ) : null)}
              {INSTALL.filter((i) => !wallets.some((w) => w.name === i.name)).map((i) => (
                <a key={i.name} className="wallet-row" href={i.href} target="_blank" rel="noreferrer">
                  <span className="wallet-icon">{i.name[0]}</span>
                  <span className="wallet-name">Install {i.name}</span>
                  <span className="small muted">{i.primary ? "recommended" : "get"}</span>
                </a>
              ))}
            </div>
            {err && <p className="small bad">{err}</p>}
          </div>
        </div>
      )}
    </>
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
