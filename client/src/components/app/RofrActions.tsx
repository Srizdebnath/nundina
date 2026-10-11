"use client";
/**
 * The ROFR decision (AE-P7): during `IssuerRofr` the session's ROFR
 * authority may take any part of any unsettled fill at the clearing price
 * (`exercise_rofr`, settled at once through the rails and the gate), or close
 * the window (`close_rofr`). Once the window has lapsed anyone may close it.
 * Signed by the connected wallet; nothing here is server-side.
 */
import React, { useEffect, useState } from "react";
import { address, type Address } from "@solana/kit";
import { explorer } from "@/lib/programs";
import { getTokenHoldings, type TokenHolding } from "@/lib/rpc";
import { fmt6, parseUnits } from "@/lib/session";
import { closeRofrIx, computeUnitLimitIx, exerciseRofrIx } from "@/lib/tx";
import { useWallet } from "@/lib/wallet";

export interface RofrProps {
  session: string;
  railsSession: string;
  assetMint: string;
  assetDecimals: number;
  usdcMint: string;
  feeReceiver: string;
  reserve: string;
  rofrAuthority: string;
  rofrEnd: number;
  clearPrice: string;
  fills: { index: number; buyer: string; seller: string; qty: string; rofr: boolean; settled: boolean }[];
}

const largest = (hs: TokenHolding[], mint: string) =>
  hs.filter((h) => h.mint === mint).sort((a, b) => (a.amount > b.amount ? -1 : 1))[0] ?? null;

export function RofrActions(p: RofrProps) {
  const { account, send } = useWallet();
  const [busy, setBusy] = useState(false);
  const [qty, setQty] = useState<Record<number, string>>({});
  const [msg, setMsg] = useState<{ ok: boolean; text: string; sig?: string } | null>(null);
  const [now, setNow] = useState(() => Math.floor(Date.now() / 1000));
  useEffect(() => {
    const t = setInterval(() => setNow(Math.floor(Date.now() / 1000)), 1000);
    return () => clearInterval(t);
  }, []);

  const isIssuer = account === p.rofrAuthority;
  const open = now < p.rofrEnd;
  const takeable = p.fills.filter((f) => !f.rofr && !f.settled && BigInt(f.qty) > 0n);

  const run = async (fn: () => Promise<string>, ok: string) => {
    setBusy(true);
    setMsg(null);
    try {
      setMsg({ ok: true, text: ok, sig: await fn() });
    } catch (e) {
      setMsg({ ok: false, text: (e as Error).message });
    } finally {
      setBusy(false);
    }
  };

  const exercise = (fillIndex: number, seller: string, max: bigint) =>
    run(async () => {
      if (!account) throw new Error("connect a wallet first");
      const q = parseUnits(qty[fillIndex] ?? fmt6(max).replace(/,/g, ""), p.assetDecimals);
      if (q <= 0n || q > max) throw new Error(`quantity must be in (0, ${fmt6(max)}]`);
      const [mine, sellers] = await Promise.all([getTokenHoldings(account), getTokenHoldings(address(seller))]);
      const issuerUsdc = largest(mine, p.usdcMint);
      const issuerAsset = largest(mine, p.assetMint);
      const sellerUsdc = largest(sellers, p.usdcMint);
      if (!issuerUsdc) throw new Error("no USDC account for the session's settlement mint in this wallet");
      if (!issuerAsset) throw new Error("no account for the asset in this wallet (create it and have the gate thaw it)");
      if (issuerAsset.frozen) throw new Error("this wallet's asset account is frozen: the gate has not admitted it");
      if (!sellerUsdc) throw new Error("the seller has no USDC account to be paid into");
      const cash = (q * BigInt(p.clearPrice) + 10n ** BigInt(p.assetDecimals) - 1n) / 10n ** BigInt(p.assetDecimals);
      if (issuerUsdc.amount < cash) throw new Error(`needs ${fmt6(cash)} USDC, wallet holds ${fmt6(issuerUsdc.amount)}`);
      const ix = await exerciseRofrIx(
        {
          session: address(p.session),
          railsSession: address(p.railsSession),
          issuer: account,
          seller: address(seller),
          assetMint: address(p.assetMint),
          usdcMint: address(p.usdcMint),
          feeReceiver: address(p.feeReceiver),
          reserve: address(p.reserve),
          sellerUsdc: sellerUsdc.account,
          issuerUsdc: issuerUsdc.account,
          issuerAsset: issuerAsset.account,
          usdcTokenProgram: issuerUsdc.program,
          assetTokenProgram: issuerAsset.program,
        },
        fillIndex,
        p.fills.length,
        q,
      );
      return send([computeUnitLimitIx(600_000), ix]);
    }, "ROFR exercised: the fill settled to the issuer at the clearing price.");

  const close = () =>
    run(async () => {
      if (!account) throw new Error("connect a wallet first");
      return send([closeRofrIx(address(p.session), account as Address)]);
    }, "ROFR window closed: the session moves to Settling.");

  return (
    <div className="panel">
      <p className="small">
        ROFR authority <code title={p.rofrAuthority}>{p.rofrAuthority.slice(0, 4)}…{p.rofrAuthority.slice(-4)}</code> ·
        clearing price {fmt6(BigInt(p.clearPrice))} USDC · window {open ? "open" : "lapsed"}
      </p>
      {!account && <p className="small muted">Connect the ROFR authority&apos;s wallet to decide.</p>}
      {account && !isIssuer && open && <p className="small muted">Only the ROFR authority can exercise or decline while the window is open.</p>}
      {isIssuer && open && (
        <>
          {takeable.length === 0 ? (
            <p className="small muted">No unsettled fill left to take.</p>
          ) : (
            <table className="t">
              <thead>
                <tr>
                  <th>fill</th>
                  <th>seller</th>
                  <th>buyer</th>
                  <th className="r">qty</th>
                  <th>take</th>
                </tr>
              </thead>
              <tbody>
                {takeable.map((f) => (
                  <tr key={f.index}>
                    <td>#{f.index}</td>
                    <td>{f.seller.slice(0, 4)}…</td>
                    <td>{f.buyer.slice(0, 4)}…</td>
                    <td className="r">{fmt6(BigInt(f.qty))}</td>
                    <td>
                      <input
                        size={8}
                        value={qty[f.index] ?? fmt6(BigInt(f.qty)).replace(/,/g, "")}
                        onChange={(e) => setQty({ ...qty, [f.index]: e.target.value })}
                      />{" "}
                      <button disabled={busy} onClick={() => void exercise(f.index, f.seller, BigInt(f.qty))}>
                        exercise
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
          <p className="small">
            <button className="ghost" disabled={busy} onClick={() => void close()}>
              decline / done: close the ROFR window
            </button>
          </p>
        </>
      )}
      {account && !open && (
        <p className="small">
          <button className="ghost" disabled={busy} onClick={() => void close()}>
            close the lapsed ROFR window
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
