"use client";
/**
 * Commit a sealed bid / reveal it, signed by the connected wallet.
 *
 * The preimage (limit, qty, nonce) never leaves the browser before the
 * reveal: it is kept in localStorage and offered as a download, because a
 * lost preimage means an unrevealed order and a forfeited bond (AE-P4).
 */
import React, { useEffect, useState } from "react";
import { address, type Address } from "@solana/kit";
import { fromHex, hex } from "@/lib/borsh";
import { explorer } from "@/lib/programs";
import { gross, parseUnits } from "@/lib/session";
import { getTokenHoldings } from "@/lib/rpc";
import { commitBidIx, commitment, revealIx } from "@/lib/tx";
import { useWallet } from "@/lib/wallet";

interface Preimage {
  side: "Buy" | "Sell";
  limit: string;
  qty: string;
  nonce: string;
}

const key = (session: string, owner: string) => `nundina:preimage:${session}:${owner}`;

function loadPreimage(session: string, owner: string): Preimage | null {
  try {
    const v = localStorage.getItem(key(session, owner));
    return v ? (JSON.parse(v) as Preimage) : null;
  } catch {
    return null;
  }
}

export function SessionActions(props: {
  session: string;
  railsSession: string;
  usdcMint: string;
  assetDecimals: number;
  phase: string;
  commitEnd: number;
  revealEnd: number;
  bond: string;
  maxCash: string;
  orders: { owner: string; revealed: boolean }[];
}) {
  const { account, send } = useWallet();
  const [limit, setLimit] = useState("0.95");
  const [qty, setQty] = useState("1");
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<{ ok: boolean; text: string; sig?: string } | null>(null);
  const [justStored, setStored] = useState<Preimage | null>(null);
  const [now, setNow] = useState(() => Math.floor(Date.now() / 1000));
  useEffect(() => {
    const t = setInterval(() => setNow(Math.floor(Date.now() / 1000)), 1000);
    return () => clearInterval(t);
  }, []);

  if (!account) return <p className="small muted">Connect a wallet to commit or reveal.</p>;
  const myOrder = props.orders.find((o) => o.owner === account) ?? null;
  const stored = justStored ?? loadPreimage(props.session, account);
  const inCommit = props.phase === "Commit" && now < props.commitEnd;
  const inReveal = props.phase === "Commit" || props.phase === "Reveal" ? now >= props.commitEnd && now < props.revealEnd : false;

  const run = async (fn: () => Promise<string>, ok: string) => {
    setBusy(true);
    setMsg(null);
    try {
      const sig = await fn();
      setMsg({ ok: true, text: ok, sig });
    } catch (e) {
      setMsg({ ok: false, text: (e as Error).message });
    } finally {
      setBusy(false);
    }
  };

  const commit = () =>
    run(async () => {
      const l = parseUnits(limit, 6);
      const q = parseUnits(qty, props.assetDecimals);
      const cash = gross(q, l, props.assetDecimals);
      if (cash > BigInt(props.maxCash)) throw new Error(`cash ${cash} exceeds the per-investor cap ${props.maxCash}`);
      const holdings = await getTokenHoldings(account);
      const usdc = holdings.filter((h) => h.mint === props.usdcMint).sort((a, b) => (a.amount > b.amount ? -1 : 1))[0];
      if (!usdc) throw new Error("no USDC token account for the session's settlement mint");
      if (usdc.amount < cash + BigInt(props.bond)) throw new Error("not enough USDC for the escrow plus the bond");
      const nonce = crypto.getRandomValues(new Uint8Array(32));
      const pre: Preimage = { side: "Buy", limit: l.toString(), qty: q.toString(), nonce: hex(nonce) };
      // Persist before sending: a committed order without its preimage cannot be revealed.
      localStorage.setItem(key(props.session, account), JSON.stringify(pre));
      setStored(pre);
      const c = await commitment("Buy", l, q, nonce, account);
      const ix = await commitBidIx(
        {
          session: address(props.session),
          railsSession: address(props.railsSession),
          owner: account,
          usdcMint: address(props.usdcMint),
          ownerUsdc: usdc.account,
          usdcTokenProgram: usdc.program,
        },
        c,
        cash,
      );
      return send([ix]);
    }, "Sealed bid committed. Keep the preimage until you reveal.");

  const reveal = (p: Preimage) =>
    run(async () => {
      const ix = await revealIx(
        address(props.session),
        address(props.railsSession),
        account as Address,
        BigInt(p.limit),
        BigInt(p.qty),
        fromHex(p.nonce),
      );
      return send([ix]);
    }, "Order revealed.");

  const download = () => {
    if (!stored) return;
    const blob = new Blob([JSON.stringify({ session: props.session, owner: account, ...stored }, null, 2)], { type: "application/json" });
    const a = document.createElement("a");
    a.href = URL.createObjectURL(blob);
    a.download = `preimage-${props.session.slice(0, 8)}.json`;
    a.click();
  };

  return (
    <div className="panel">
      {inCommit && !myOrder && (
        <form
          className="row"
          onSubmit={(e) => {
            e.preventDefault();
            void commit();
          }}
        >
          <label>
            limit (USDC / unit)
            <input value={limit} onChange={(e) => setLimit(e.target.value)} size={10} />
          </label>
          <label>
            quantity (units)
            <input value={qty} onChange={(e) => setQty(e.target.value)} size={10} />
          </label>
          <button disabled={busy}>commit sealed bid</button>
          <span className="small muted">escrows limit × qty plus a bond of {Number(props.bond) / 1e6} USDC in payment-rails</span>
        </form>
      )}
      {inCommit && myOrder && <p className="small">Your order is committed and sealed. Reveal opens when the commit window closes.</p>}
      {inReveal && myOrder && !myOrder.revealed && (
        <div className="row" style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
          {stored ? (
            <>
              <span className="small">
                stored preimage: limit {Number(stored.limit) / 1e6} · qty {Number(stored.qty) / 10 ** props.assetDecimals}
              </span>
              <button disabled={busy} onClick={() => void reveal(stored)}>
                reveal
              </button>
            </>
          ) : (
            <PasteReveal onReveal={(p) => void reveal(p)} busy={busy} />
          )}
        </div>
      )}
      {myOrder?.revealed && <p className="small ok">Your order is revealed.</p>}
      {!inCommit && !inReveal && !myOrder && <p className="small muted">This session is not taking orders.</p>}
      {stored && (
        <p className="small">
          <button className="ghost" onClick={download}>
            download preimage backup
          </button>
        </p>
      )}
      {msg && (
        <p className={`small ${msg.ok ? "ok" : "bad"}`}>
          {msg.text}{" "}
          {msg.sig && (
            <a href={explorer("tx", msg.sig)} target="_blank" rel="noreferrer">
              {msg.sig.slice(0, 12)}…
            </a>
          )}
        </p>
      )}
    </div>
  );
}

function PasteReveal({ onReveal, busy }: { onReveal: (p: Preimage) => void; busy: boolean }) {
  const [text, setText] = useState("");
  const [err, setErr] = useState<string | null>(null);
  return (
    <form
      className="row"
      onSubmit={(e) => {
        e.preventDefault();
        try {
          const p = JSON.parse(text) as Preimage;
          if (!p.limit || !p.qty || !p.nonce) throw new Error("needs limit, qty, nonce");
          onReveal(p);
        } catch (x) {
          setErr((x as Error).message);
        }
      }}
    >
      <label>
        no stored preimage — paste the backup JSON
        <input value={text} onChange={(e) => setText(e.target.value)} size={60} />
      </label>
      <button disabled={busy}>reveal</button>
      {err && <span className="bad small">{err}</span>}
    </form>
  );
}
