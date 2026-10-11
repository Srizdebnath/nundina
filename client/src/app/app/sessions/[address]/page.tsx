import React from "react";
import Link from "next/link";
import { address, isAddress } from "@solana/kit";
import { Addr, Banner, ErrorPanel, Stat, utc } from "@/components/app/ui";
import { AutoRefresh, Countdown } from "@/components/app/client";
import { ClearingChart } from "@/components/app/ClearingChart";
import { SessionActions } from "@/components/app/SessionActions";
import { RofrActions } from "@/components/app/RofrActions";
import { CrankPanel } from "@/components/app/CrankPanel";
import { hex, type Struct } from "@/lib/borsh";
import { chainNow } from "@/lib/rpc";
import { clockPhase, curves, fmt6, loadSession, navToPrice, PHASES, type SessionView } from "@/lib/session";
import { disclose, enumName, type PrintPolicy } from "@/lib/prints";
import { gateStatus } from "@/lib/gate";
import { TOKEN_2022_PROGRAM, TOKEN_PROGRAM } from "@/lib/programs";
import { connection } from "next/server";


function Timeline({ phase }: { phase: string }) {
  const at = PHASES.indexOf(phase as (typeof PHASES)[number]);
  return (
    <div className="phases">
      {PHASES.map((p, i) => (
        <span key={p} className={`phase ${phase === "Failed" ? "" : i < at ? "done" : i === at ? "now" : ""}`}>
          {p}
        </span>
      ))}
      {phase === "Failed" && <span className="phase fail">Failed</span>}
    </div>
  );
}

export default async function SessionPage({ params }: { params: Promise<{ address: string }> }) {
  await connection();
  const { address: raw } = await params;
  if (!isAddress(raw)) {
    return (
      <main className="wrap">
        <ErrorPanel title="Not an address">{raw}</ErrorPanel>
      </main>
    );
  }
  const s = await loadSession(address(raw));
  if (!s) {
    return (
      <main className="wrap">
        <Banner />
        <ErrorPanel title="No auction-engine session at this address">
          <Addr a={raw} /> — <Link href="/app/sessions">all sessions</Link>
        </ErrorPanel>
      </main>
    );
  }
  const now = await chainNow();
  const p = s.params;
  const commitEnd = Number(p.commit_end as bigint);
  const revealEnd = Number(p.reveal_end as bigint);
  const deadline = Number(p.settle_deadline as bigint);
  const due = clockPhase(p, s.phase, now);
  const { demand, supply } = curves(s.orders);
  const nav = s.feedState ? navToPrice(s.feedState.nav_pico as bigint, s.assetDecimals) : null;
  const discountBps = nav && s.clearPrice > 0n ? Number(((nav - s.clearPrice) * 10_000n) / nav) : null;
  const unit = 10n ** BigInt(s.assetDecimals);
  const usdcMint = (s.rails?.usdc_mint as string | undefined) ?? "";

  // Gate market + each order owner's `investor_id`, resolved once on the server
  // so the crank can build `screen_bid` and `settle` without a client round-trip
  // per order. Missing ids are simply absent from the map; the crank then refuses
  // that step with a readable error instead of failing on chain.
  const owners = [...new Set(s.orders.map((o) => o.owner))];
  const statuses = await Promise.all(owners.map((o) => gateStatus(s.assetMint, o)));
  const first = statuses.find((g) => g.market);
  const gateInfo = {
    credential: (first?.market?.policy.credential as string) ?? "",
    identitySchema: (first?.market?.config.identity_schema as string) ?? "",
    accreditationSchema: (first?.market?.config.accreditation_schema as string) ?? "",
    investorIds: Object.fromEntries(
      statuses
        .filter((g) => g.identity.parsed?.investorId)
        .map((g) => [g.wallet, Buffer.from(g.identity.parsed!.investorId as Uint8Array).toString("hex")]),
    ),
  };
  const live = s.phase !== "Printed" && s.phase !== "Failed";
  return (
    <main className="wrap">
      {live && <AutoRefresh seconds={5} />}
      <Banner />
      <h1>
        Session #{String(s.id)} {(p.forced_sale as boolean) && <span className="pill bad">forced sale</span>}
      </h1>
      <p className="small muted">
        <Addr a={s.address} /> · asset <Addr a={s.assetMint} /> · rails session <Addr a={s.railsSession} /> · feed{" "}
        <Addr a={s.feed} />
      </p>
      <Timeline phase={s.phase} />
      {due !== s.phase && (
        <p className="small warn">
          The clock says {due}; the on-chain phase moves when a crank (or anyone) sends the next phase instruction.
        </p>
      )}

      <div className="grid" style={{ marginTop: 14 }}>
        <Stat k="commit closes" v={<Countdown to={commitEnd} done={utc(commitEnd)} />} />
        <Stat k="reveal closes" v={<Countdown to={revealEnd} done={utc(revealEnd)} />} />
        <Stat k="settle deadline" v={<Countdown to={deadline} done={utc(deadline)} />} />
        <Stat k="orders (revealed)" v={`${s.orders.length} (${s.orders.filter((o) => o.revealed).length})`} />
      </div>

      <h2>Clearing</h2>
      <div className="grid">
        <Stat k="NAV (mark-engine)" v={nav !== null ? fmt6(nav, 4) : "—"} />
        <Stat k={`reserve (NAV − ${Number(p.max_discount_bps as bigint) / 100}%)`} v={s.reservePrice > 0n ? fmt6(s.reservePrice, 4) : "—"} />
        <Stat k="clearing price" v={s.clearPrice > 0n ? fmt6(s.clearPrice, 4) : "—"} cls={s.clearPrice > 0n ? "ok" : "muted"} />
        <Stat k="discount to NAV" v={discountBps !== null ? `${(discountBps / 100).toFixed(2)}%` : "—"} />
        <Stat k="cleared volume" v={s.clearedVolume > 0n ? fmt6(s.clearedVolume, 2) : "—"} />
        <Stat k="fills settled" v={`${s.fillsSettled} / ${s.fills.length}`} />
      </div>
      <div className="panel" style={{ marginTop: 12 }}>
        <ClearingChart
          demand={demand}
          supply={supply}
          clearPrice={s.clearPrice}
          clearedVolume={s.clearedVolume}
          reserve={s.reservePrice}
          nav={nav}
        />
      </div>

      <h2>Your order</h2>
      <SessionActions
        session={s.address}
        railsSession={s.railsSession}
        usdcMint={usdcMint}
        assetDecimals={s.assetDecimals}
        phase={s.phase}
        commitEnd={commitEnd}
        revealEnd={revealEnd}
        bond={String(p.bond as bigint)}
        maxCash={String(p.max_cash as bigint)}
        orders={s.orders.map((o) => ({ owner: o.owner, revealed: o.revealed }))}
        assetMint={s.assetMint}
        usdcTokenProgram={(s.rails?.usdc_token_program as string) ?? TOKEN_PROGRAM}
        assetTokenProgram={TOKEN_2022_PROGRAM}
      />

      {s.rails && gateInfo.credential && (
        <CrankPanel
          session={s.address}
          railsSession={s.railsSession}
          feed={s.feed}
          assetMint={s.assetMint}
          usdcMint={usdcMint}
          phase={s.phase}
          assetDecimals={s.assetDecimals}
          commitEnd={commitEnd}
          revealEnd={revealEnd}
          settleDeadline={deadline}
          now={now}
          clearPrice={String(s.clearPrice)}
          orders={s.orders.map((o) => ({
            owner: o.owner,
            side: o.side,
            revealed: o.revealed,
            screened: o.screened,
            eligible: o.eligible,
          }))}
          fills={s.fills.map((f, i) => ({
            index: i,
            buyer: f.buyer,
            seller: f.seller,
            qty: String(f.qty),
            settled: f.settled,
          }))}
          feeReceiver={s.rails.fee_receiver as string}
          reserve={s.rails.reserve as string}
          buyerTokenProgram={s.rails.usdc_token_program as string}
          assetTokenProgram={TOKEN_2022_PROGRAM}
          printAuthority={(first?.market?.config.gate_config_authority as string) ?? ""}
          gate={gateInfo}
        />
      )}

      <h2>Order book</h2>
      <table className="t">
        <thead>
          <tr>
            <th>owner</th>
            <th>side</th>
            <th>commitment</th>
            <th className="r">escrowed</th>
            <th className="r">limit</th>
            <th className="r">qty</th>
            <th>gate check</th>
            <th className="r">allocated</th>
          </tr>
        </thead>
        <tbody>
          {s.orders.map((o, i) => (
            <tr key={i}>
              <td>
                <Link href={`/app/portfolio?wallet=${o.owner}`}>{o.owner.slice(0, 4)}…{o.owner.slice(-4)}</Link>
              </td>
              <td>{o.side}</td>
              <td className="small muted">{hex(o.commitment).slice(0, 16)}…</td>
              <td className="r">{o.side === "Buy" ? `${fmt6(o.escrowed)} USDC` : fmt6(o.escrowed, 2)}</td>
              <td className="r">{o.revealed ? fmt6(o.limit, 4) : <span className="muted">sealed</span>}</td>
              <td className="r">{o.revealed ? fmt6((o.qty * 1_000_000n) / unit, 2) : <span className="muted">sealed</span>}</td>
              <td className="small">
                {!o.revealed ? <span className="muted">—</span> : !o.screened ? <span className="muted">pending</span> : o.eligible ? "eligible" : <span className="bad">excluded</span>}
              </td>
              <td className="r">{o.alloc > 0n ? fmt6((o.alloc * 1_000_000n) / unit, 2) : "—"}</td>
            </tr>
          ))}
        </tbody>
      </table>

      <h2>Fills (DvP through payment-rails + credit-gate settle_check)</h2>
      {s.fills.length === 0 ? (
        <p className="muted small">No fills allocated yet.</p>
      ) : (
        <table className="t">
          <thead>
            <tr>
              <th>#</th>
              <th>buyer</th>
              <th>seller</th>
              <th className="r">qty</th>
              <th className="r">cash at clear</th>
              <th>ROFR</th>
              <th>settled</th>
            </tr>
          </thead>
          <tbody>
            {s.fills.map((f) => (
              <tr key={f.index}>
                <td>{f.index}</td>
                <td>
                  <Addr a={f.buyer} />
                </td>
                <td>
                  <Addr a={f.seller} />
                </td>
                <td className="r">{fmt6((f.qty * 1_000_000n) / unit, 2)}</td>
                <td className="r">{fmt6((f.qty * s.clearPrice) / unit)}</td>
                <td>{f.rofr ? "issuer" : "—"}</td>
                <td className={f.settled ? "ok" : "warn"}>{f.settled ? "settled" : "pending"}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      {s.phase === "IssuerRofr" && s.rails && (
        <>
          <h2>Issuer ROFR decision (AE-P7)</h2>
          <RofrActions
            session={s.address}
            railsSession={s.railsSession}
            assetMint={s.assetMint}
            assetDecimals={s.assetDecimals}
            usdcMint={usdcMint}
            feeReceiver={s.rails.fee_receiver as string}
            reserve={s.rails.reserve as string}
            rofrAuthority={p.rofr_authority as string}
            rofrEnd={revealEnd + Number(p.rofr_window as bigint)}
            clearPrice={String(s.clearPrice)}
            fills={s.fills.map((f) => ({ ...f, qty: String(f.qty) }))}
          />
        </>
      )}

      <h2>Print</h2>
      {s.print ? (
        <PrintLine s={s} />
      ) : (
        <p className="muted small">Not printed yet: `finish` records the print in mark-engine.</p>
      )}

      {s.rails && (
        <>
          <h2>Cash leg (payment-rails CashSession)</h2>
          <RailsSummary r={s.rails} />
        </>
      )}
    </main>
  );
}

function RailsSummary({ r }: { r: Struct }) {
  return (
    <div className="grid">
      <Stat k="status" v={String(r.status)} />
      <Stat k="open escrows" v={String(r.open_escrows)} />
      <Stat k="cash escrowed" v={fmt6(r.total_cash as bigint)} />
      <Stat k="bonds" v={fmt6(r.total_bonds as bigint)} />
      <Stat k="gross settled" v={fmt6(r.gross_settled as bigint)} />
      <Stat k="shortfalls (cash / asset)" v={`${fmt6(r.total_cash_shortfall as bigint)} / ${fmt6(r.total_asset_shortfall as bigint)}`} />
    </div>
  );
}

/** The print as its market's class discloses it (ME-P6); the full record is in the data room. */
function PrintLine({ s }: { s: SessionView }) {
  const pr = s.print!;
  const params = (s.feedState?.params ?? {}) as Struct;
  const policy = (s.feedState ? enumName(params.print_policy) : "DataRoom") as PrintPolicy;
  const d = disclose(policy, pr.price as bigint, pr.volume as bigint, (params.band_bps as bigint | undefined) ?? 0n);
  return (
    <p className="small">
      {d.price !== null ? fmt6(d.price, 4) : "price withheld"} × {d.volume !== null ? fmt6(d.volume, 2) : "volume withheld"} ·{" "}
      {pr.n_investor_ids as number} investors · {enumName(pr.eligibility)} · recorded {utc(Number(pr.recorded_at as bigint))} · class{" "}
      {policy} · <Link href="/app/data-room">full record in the data room</Link>
    </p>
  );
}
