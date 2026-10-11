"use client";
/**
 * The crank: one button that decides the next step of a sealed-bid session and
 * shows a preflight of exactly what it will do before anything is signed.
 *
 * The architecture (doc 06 §8) puts phase cranks on a bonded keeper, and that
 * is still the production path — `payment-rails/crank` drives the same
 * instructions. This panel is the same crank run by hand from the browser: the
 * same instructions, the same account blocks, permissionlessly, so a session
 * can be driven to a clearing price when no keeper is running. That matters for
 * a live demo, and it doubles as the thing you point at to show the state
 * machine: every step is visible, in order, with the reason it is legal now.
 *
 * Steps and why each is or isn't available:
 *   screen → `screen_bid` / `screen_ask`   AE-P6, permissionless, safe to repeat
 *   clear  → `clear`                        AE-P5, needs a crossed book above the floor
 *   allocate → `allocate`                   AE-P6, chunked and resumable
 *   settle → `settle(fill)`                 AE-P8, one fill per instruction (DvP)
 *   finish → `finish`                       AE-P9, writes the clearing print
 *   expire → `expire`                       AE-P10, refunds everything and fails
 */
import React, { useState } from "react";
import { address, type Instruction } from "@solana/kit";
import { explorer } from "@/lib/programs";
import { useWallet } from "@/lib/wallet";
import {
  allocateIx,
  clearIx,
  computeUnitLimitIx,
  expireIx,
  finishIx,
  screenAskIx,
  screenBidIx,
  settleIx,
} from "@/lib/tx";

type StepId = "screen" | "clear" | "allocate" | "settle" | "finish" | "expire";

export interface CrankOrder {
  owner: string;
  side: "Buy" | "Sell";
  revealed: boolean;
  screened: boolean;
  eligible: boolean;
}
export interface CrankFill {
  index: number;
  buyer: string;
  seller: string;
  qty: string;
  /** Not stored per fill on the engine; shown when the caller has it. */
  price?: string;
  settled: boolean;
}

export interface CrankProps {
  session: string;
  railsSession: string;
  feed: string;
  assetMint: string;
  usdcMint: string;
  phase: string;
  assetDecimals: number;
  commitEnd: number;
  revealEnd: number;
  settleDeadline: number;
  /** 0 until `clear` has run. */
  clearPrice: string;
  now: number;
  orders: CrankOrder[];
  fills: CrankFill[];
  /** Addresses the crank needs that live on `RailsConfig` / the gate market. */
  feeReceiver: string;
  reserve: string;
  buyerTokenProgram: string;
  assetTokenProgram: string;
  printAuthority: string;
  gate: {
    credential: string;
    identitySchema: string;
    accreditationSchema: string;
    /** investor_id per wallet, read from each identity attestation. */
    investorIds: Record<string, string>;
  };
}

/** What `clear` will do, computed the same way the engine computes it. */
function planSteps(p: CrankProps): { id: StepId; label: string; why: string; ready: boolean; detail: string }[] {
  const unscreened = p.orders.filter((o) => o.revealed && !o.screened);
  const revealed = p.orders.filter((o) => o.revealed);
  const unsettled = p.fills.filter((f) => !f.settled);
  const pastReveal = p.now >= p.revealEnd;
  const pastSettle = p.now >= p.settleDeadline;
  const printed = p.phase === "Printed";

  return [
    {
      id: "screen",
      label: "screen orders",
      why: "AE-P6 — per-order credit-gate check. An ineligible order is excluded, never an error.",
      ready: unscreened.length > 0 && !printed,
      detail: unscreened.length
        ? `${unscreened.length} revealed order(s) still unscreened`
        : "every revealed order is already screened",
    },
    {
      id: "clear",
      label: "clear",
      why: "AE-P5 — find the uniform price maximising executed volume, against the rails reserve floor.",
      ready: pastReveal && unscreened.length === 0 && revealed.length > 0 && p.clearPrice !== "0",
      detail:
        p.clearPrice !== "0"
          ? `already cleared at ${p.clearPrice}`
          : !pastReveal
            ? "reveal window is still open"
            : unscreened.length
              ? "screen the book first"
              : `${revealed.length} revealed order(s), no price yet`,
    },
    {
      id: "allocate",
      label: "allocate",
      why: "AE-P6 — pair the crossed book into fills, pro-rata at the margin, chunked and resumable.",
      ready: p.clearPrice !== "0" && unsettled.length > 0 && p.phase !== "Clearing",
      detail: unsettled.length ? `${unsettled.length} fill(s) outstanding` : "every fill is allocated and settled",
    },
    {
      id: "settle",
      label: "settle next fill",
      why: "AE-P8 — ONE fill, atomic DvP. The rails re-check credit-gate settle_check before any value moves.",
      ready: p.clearPrice !== "0" && unsettled.length > 0,
      detail: unsettled.length
        ? `fill #${unsettled[0]!.index}: ${unsettled[0]!.qty} units`
        : "nothing left to settle",
    },
    {
      id: "finish",
      label: "finish + print",
      why: "AE-P9 — close the book, refund the unfilled escrow, write the clearing print into mark-engine.",
      ready: p.clearPrice !== "0" && unsettled.length === 0 && !printed,
      detail: printed ? "already printed" : unsettled.length ? "settle every fill first" : "book is settled — safe to print",
    },
    {
      id: "expire",
      label: "expire + refund",
      why: "AE-P10 — past the deadline: refund every participant from the rails, in full, permissionlessly.",
      ready: pastSettle && !printed,
      detail: pastSettle ? "past the settlement deadline" : `deadline at ${new Date(p.settleDeadline * 1000).toISOString()}`,
    },
  ];
}

export function CrankPanel(p: CrankProps) {
  const { account, send } = useWallet();
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<{ ok: boolean; text: string; sig?: string } | null>(null);
  const [open, setOpen] = useState(false);

  const steps = planSteps(p);
  const next = steps.find((s) => s.ready && s.id !== "expire");
  const expire = steps.find((s) => s.id === "expire")!;

  const run = async (fn: () => Promise<Instruction[]>, ok: string) => {
    setBusy(true);
    setMsg(null);
    try {
      const sig = await send(fn as never);
      setMsg({ ok: true, text: ok, sig });
    } catch (e) {
      setMsg({ ok: false, text: (e as Error).message });
    } finally {
      setBusy(false);
    }
  };

  const A_ = (s: string) => address(s);
  const a = (s: string) => A_(s);

  /** Build the instruction(s) for one step. Pure — the preflight calls it too. */
  async function build(id: StepId): Promise<Instruction[]> {
    const session = a(p.session);
    switch (id) {
      case "screen": {
        const out: Instruction[] = [];
        for (const o of p.orders.filter((x) => x.revealed && !x.screened)) {
          const ix = o.side === "Buy" ? await screenBidIxPromise(o) : await screenAskIxPromise(o);
          if (ix) out.push(ix);
        }
        return out;
      }
      case "clear":
        return [await clearIx(session, a(p.feed))];
      case "allocate":
        return [allocateIx(session, 32)];
      case "settle": {
        const f = p.fills.find((x) => !x.settled);
        if (!f) throw new Error("nothing to settle");
        const inv = p.gate.investorIds[f.buyer];
        if (!inv) throw new Error(`no investor_id known for buyer ${f.buyer.slice(0, 8)}… — they have no identity attestation`);
        const ix = await settleIx(
          {
            session,
            railsSession: a(p.railsSession),
            assetMint: a(p.assetMint),
            usdcMint: a(p.usdcMint),
            buyer: a(f.buyer),
            seller: a(f.seller),
            buyerAsset: a(p.feeReceiver),
            sellerUsdc: a(p.feeReceiver),
            feeReceiver: a(p.feeReceiver),
            reserve: a(p.reserve),
            buyerTokenProgram: a(p.buyerTokenProgram),
            assetTokenProgram: a(p.assetTokenProgram),
            payer: account!,
            buyerInvestorId: Uint8Array.from(Buffer.from(inv, "hex")),
            credential: a(p.gate.credential),
            identitySchema: a(p.gate.identitySchema),
            accreditationSchema: a(p.gate.accreditationSchema),
          },
          f.index,
        );
        // DvP through rails + gate needs well over the 200k default.
        return [computeUnitLimitIx(600_000), ix];
      }
      case "finish":
        return [
          await finishIx({
            session,
            feed: a(p.feed),
            assetMint: a(p.assetMint),
            printAuthority: a(p.printAuthority),
            payer: account!,
          }),
        ];
      case "expire":
        return [await expireIx(session, a(p.feed))];
    }
  }

  async function screenBidIxPromise(o: CrankOrder) {
    const inv = p.gate.investorIds[o.owner];
    if (!inv) return null;
    return screenBidIx({
      session: a(p.session),
      assetMint: a(p.assetMint),
      buyer: a(o.owner),
      orderIndex: p.orders.indexOf(o),
      investorId: Uint8Array.from(Buffer.from(inv, "hex")),
      credential: a(p.gate.credential),
      identitySchema: a(p.gate.identitySchema),
      accreditationSchema: a(p.gate.accreditationSchema),
    });
  }
  async function screenAskIxPromise(o: CrankOrder) {
    return screenAskIx({
      session: a(p.session),
      assetMint: a(p.assetMint),
      seller: a(o.owner),
      orderIndex: p.orders.indexOf(o),
    });
  }

  if (!account) return null;

  return (
    <div className="panel">
      <div className="row" style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
        <strong>crank</strong>
        <span className="small muted">
          the same instructions a bonded keeper runs, run by hand. every step is permissionless except
          `settle` and `finish`, which need a payer.
        </span>
      </div>

      {!next && !expire.ready && <p className="small muted">nothing to crank — this session has run its course.</p>}

      {next && (
        <div className="row" style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
          <button
            disabled={busy}
            onClick={() => void run(() => build(next.id), `${next.label} done.`)}
          >
            {busy ? "working…" : `run next step — ${next.label}`}
          </button>
          <button className="ghost" onClick={() => setOpen((v) => !v)}>
            {open ? "hide preflight" : "what will this do?"}
          </button>
        </div>
      )}

      {open && (
        <div className="small" style={{ marginTop: 8 }}>
          <table style={{ width: "100%", borderCollapse: "collapse" }}>
            <thead>
              <tr>
                <th style={{ textAlign: "left" }}>step</th>
                <th style={{ textAlign: "left" }}>state</th>
                <th style={{ textAlign: "left" }}>why</th>
              </tr>
            </thead>
            <tbody>
              {steps.map((s) => (
                <tr key={s.id} style={{ opacity: s.ready ? 1 : 0.5 }}>
                  <td style={{ paddingRight: 8 }}>
                    {s.id === next?.id ? <strong>{s.label}</strong> : s.label}
                  </td>
                  <td style={{ paddingRight: 8 }}>
                    {s.ready ? <span className="ok">ready</span> : <span className="muted">{s.detail}</span>}
                  </td>
                  <td className="muted">{s.why}</td>
                </tr>
              ))}
            </tbody>
          </table>
          {expire.ready && (
            <p>
              <button className="ghost" disabled={busy} onClick={() => void run(() => build("expire"), "Session expired, escrow refunded.")}>
                expire + refund everything
              </button>{" "}
              <span className="muted">{expire.why}</span>
            </p>
          )}
        </div>
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
