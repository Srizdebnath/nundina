import React from "react";
import Link from "next/link";
import { address, isAddress } from "@solana/kit";
import { Addr, Banner, ErrorPanel, Stat, utc } from "@/components/ui";
import { AutoRefresh, Countdown, WalletLookup } from "@/components/client";
import { NPCS_MINT } from "@/lib/programs";
import { chainNow } from "@/lib/rpc";
import { loadPortfolio, type Portfolio } from "@/lib/portfolio";
import { fmt6 } from "@/lib/session";
import type { Struct } from "@/lib/borsh";

export const dynamic = "force-dynamic";

const n = (v: unknown) => Number(v as bigint);

function Lots({ p, now }: { p: Portfolio; now: number }) {
  if (!p.lots) return <p className="muted small">No lot ledger: this wallet has not been validated by the gate.</p>;
  return (
    <>
      <p className="small">
        Free (seasoned, sellable): <b>{fmt6(p.lots.freeQty)}</b>
      </p>
      {p.lots.lots.length === 0 ? (
        <p className="muted small">No lots still seasoning.</p>
      ) : (
        <table className="t">
          <thead>
            <tr>
              <th>lot</th>
              <th className="r">qty</th>
              <th>source</th>
              <th>acquired</th>
              <th>sellable from</th>
              <th>seasoning</th>
            </tr>
          </thead>
          <tbody>
            {p.lots.lots.map((l) => (
              <tr key={l.lotId}>
                <td>#{l.lotId}</td>
                <td className="r">{fmt6(l.qty)}</td>
                <td>{l.source}</td>
                <td>{utc(l.acquiredTs)}</td>
                <td>{utc(l.restrictionExpiry)}</td>
                <td className={l.restrictionExpiry <= now ? "ok" : "warn"}>
                  <Countdown to={l.restrictionExpiry} done="seasoned — merge_expired moves it to free" />
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </>
  );
}

function stateName(v: unknown): string {
  return typeof v === "string" ? v : Object.keys(v as Struct)[0] ?? "?";
}

function Claims({ p }: { p: Portfolio }) {
  if (p.claims.length === 0) return <p className="muted small">No redemption-queue claims.</p>;
  return (
    <table className="t">
      <thead>
        <tr>
          <th>claim</th>
          <th className="r">qty</th>
          <th>window</th>
          <th>state</th>
          <th className="r">filled</th>
          <th className="r">cash received</th>
          <th>transferable / pledgeable</th>
          <th>pledge</th>
        </tr>
      </thead>
      <tbody>
        {p.claims.map(({ address: a, request: r }) => (
          <tr key={a}>
            <td>
              <Addr a={a} /> #{String(r.id)}
            </td>
            <td className="r">{fmt6(r.qty as bigint)}</td>
            <td>{utc(n(r.window_date))}</td>
            <td>{stateName(r.state)}</td>
            <td className="r">{fmt6(r.filled_qty as bigint)}</td>
            <td className="r">{fmt6(r.cash_received as bigint)}</td>
            <td>
              {(r.transferable as boolean) ? "yes" : "no"} / {(r.pledgeable as boolean) ? "yes" : "no"}
            </td>
            <td>{r.pledge ? <>facility <Addr a={(r.pledge as Struct).facility_config as string} /></> : "—"}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function Advances({ p }: { p: Portfolio }) {
  if (p.positions.length === 0) return <p className="muted small">No facility advances.</p>;
  return (
    <table className="t">
      <thead>
        <tr>
          <th>position</th>
          <th className="r">collateral</th>
          <th className="r">advanced (USDC)</th>
          <th>opened</th>
          <th>state</th>
          <th className="r">shortfall</th>
        </tr>
      </thead>
      <tbody>
        {p.positions.map(({ address: a, position: x }) => (
          <tr key={a}>
            <td>
              <Addr a={a} />
            </td>
            <td className="r">{fmt6(x.qty as bigint)}</td>
            <td className="r">{fmt6(x.advanced as bigint)}</td>
            <td>{utc(n(x.opened_at))}</td>
            <td>{stateName(x.state)}</td>
            <td className="r">{fmt6(x.shortfall as bigint)}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function Tranches({ p }: { p: Portfolio }) {
  if (p.tranches.length === 0) return <p className="muted small">No tranche-engine positions.</p>;
  return (
    <table className="t">
      <thead>
        <tr>
          <th>pool</th>
          <th>state</th>
          <th>tranche</th>
          <th className="r">units</th>
          <th className="r">share</th>
          <th className="r">principal</th>
          <th className="r">allocated loss</th>
          <th className="r">coupon</th>
          <th>lock-up</th>
        </tr>
      </thead>
      <tbody>
        {p.tranches.map((t) => (
          <tr key={`${t.pool}-${t.side}`}>
            <td>
              <Addr a={t.pool} /> #{t.poolId}
            </td>
            <td>{t.state}</td>
            <td>{t.side}</td>
            <td className="r">{fmt6(t.amount)}</td>
            <td className="r">{(Number(t.shareBps) / 100).toFixed(2)}%</td>
            <td className="r">{fmt6(t.principal)}</td>
            <td className={`r ${t.loss > 0n ? "bad" : ""}`}>{fmt6(t.loss)}</td>
            <td className="r">{(t.couponBps / 100).toFixed(2)}%</td>
            <td>{t.locked ? "locked (frozen)" : "—"}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

export default async function PortfolioPage({ searchParams }: { searchParams: Promise<{ wallet?: string; mint?: string }> }) {
  const q = await searchParams;
  const wallet = q.wallet && isAddress(q.wallet) ? address(q.wallet) : null;
  const mint = q.mint && isAddress(q.mint) ? address(q.mint) : NPCS_MINT;
  let p: Portfolio | null = null;
  let failure: string | null = null;
  let now = 0;
  if (wallet) {
    try {
      [p, now] = await Promise.all([loadPortfolio(wallet, mint), chainNow()]);
    } catch (e) {
      failure = (e as Error).message;
    }
  }
  const npcs = p?.holdings.filter((h) => h.mint === mint) ?? [];
  const usdc = p?.holdings.filter((h) => h.mint !== mint && h.decimals === 6) ?? [];
  return (
    <main className="wrap">
      <Banner />
      <h1>Portfolio</h1>
      <p className="muted small">
        Lots and their seasoning countdown, redemption-queue claims, facility advances and auction orders for one wallet,
        read from the programs on this cluster.
      </p>
      <WalletLookup current={wallet} />
      {q.wallet && !wallet && <ErrorPanel title="Not an address">{q.wallet}</ErrorPanel>}
      {failure && <ErrorPanel title="RPC error">{failure}</ErrorPanel>}
      {p && (
        <>
          <AutoRefresh seconds={15} />
          <h2>Holdings</h2>
          <div className="grid">
            <Stat k="NPCS" v={npcs.length ? fmt6(npcs.reduce((s, h) => s + h.amount, 0n)) : "—"} />
            <Stat
              k="NPCS account"
              v={npcs.length ? (npcs.every((h) => !h.frozen) ? "thawed" : "frozen") : "none"}
              cls={npcs.length ? (npcs.every((h) => !h.frozen) ? "ok" : "bad") : "muted"}
            />
            <Stat k="gate" v={p.link ? "admitted" : "not admitted"} cls={p.link ? "ok" : "bad"} />
            <Stat k="6-dp tokens (USDC)" v={usdc.length ? fmt6(usdc.reduce((s, h) => s + h.amount, 0n)) : "—"} />
          </div>
          {!p.link && (
            <p className="small">
              <Link href={`/gate?wallet=${p.wallet}`}>Why is this wallet not admitted? →</Link>
            </p>
          )}
          <h2>Lots (CG-P3 seasoning)</h2>
          <Lots p={p} now={now} />
          <h2>Claims (queue-claim)</h2>
          <Claims p={p} />
          <h2>Advances (liquidity-facility)</h2>
          <Advances p={p} />
          <h2>Tranche positions (tranche-engine)</h2>
          <Tranches p={p} />
          <h2>Auction orders</h2>
          {p.orders.length === 0 ? (
            <p className="muted small">No orders in any session.</p>
          ) : (
            <table className="t">
              <thead>
                <tr>
                  <th>session</th>
                  <th>phase</th>
                  <th>side</th>
                  <th className="r">escrowed</th>
                  <th>revealed</th>
                  <th className="r">limit</th>
                  <th className="r">qty</th>
                  <th className="r">allocated</th>
                </tr>
              </thead>
              <tbody>
                {p.orders.map((o) => (
                  <tr key={`${o.session}-${o.side}`}>
                    <td>
                      <Link href={`/sessions/${o.session}`}>#{String(o.sessionId)}</Link>
                    </td>
                    <td>{o.phase}</td>
                    <td>{o.side}</td>
                    <td className="r">{fmt6(o.escrowed)}</td>
                    <td>{o.revealed ? "yes" : "sealed"}</td>
                    <td className="r">{o.revealed ? fmt6(o.limit) : "—"}</td>
                    <td className="r">{o.revealed ? fmt6(o.qty) : "—"}</td>
                    <td className="r">{fmt6(o.alloc)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </>
      )}
    </main>
  );
}
