import React from "react";
import { address, isAddress, type Address } from "@solana/kit";
import { Addr, Banner, ErrorPanel, Stat, utc } from "@/components/app/ui";
import { AutoRefresh, WalletLookup } from "@/components/app/client";
import { FacilityActions, type FacilityProps } from "@/components/app/FlowActions";
import { loadBorrower, loadFacilities, loadPayout, mintInfos, type FacilityRow } from "@/lib/flows-read";
import { PROGRAMS } from "@/lib/programs";
import { chainNow } from "@/lib/rpc";
import { fmt6 } from "@/lib/session";
import type { Struct } from "@/lib/borsh";
import { connection } from "next/server";


const pct = (bps: unknown) => `${(Number(bps as bigint) / 100).toFixed(2)}%`;
const stateName = (v: unknown) => (typeof v === "string" ? v : (Object.keys(v as Struct)[0] ?? "?"));

/** Why advances cannot be paid to this wallet (rails `PayoutDestination::ensure_payable`), if they cannot. */
function payoutProblem(d: Struct | null, now: number): string | null {
  if (!d) return "No payout destination: the rails registrar registers one (attested owner + token account) before any advance.";
  if (!(d.active as boolean)) return "Your payout destination is revoked.";
  if (Number(d.expires_at as bigint) <= now) return `Your payout destination expired ${utc(Number(d.expires_at as bigint))}.`;
  return null;
}

async function Facility({ f, wallet, now }: { f: FacilityRow; wallet: Address | null; now: number }) {
  const c = f.config;
  const params = c.params as Struct;
  const [asset, usdc] = await mintInfos([c.asset_mint as Address, c.usdc_mint as Address]);
  let actions: FacilityProps | null = null;
  let position: Struct | null = null;
  if (wallet && asset && usdc) {
    const [b, payout] = await Promise.all([loadBorrower(f.address, c.rails_facility as Address, wallet), loadPayout(wallet)]);
    position = b.state;
    const problem = payoutProblem(payout, now);
    actions = {
      forWallet: wallet,
      ref_: {
        config: f.address,
        facilityId: String(c.facility_id as bigint),
        assetMint: asset.address,
        usdcMint: usdc.address,
        feed: c.feed as string,
        assetTokenProgram: asset.program,
        usdcTokenProgram: usdc.program,
      },
      assetDecimals: asset.decimals,
      rateBps: String(params.rate_bps as bigint),
      advanceRateBps: String(params.advance_rate_bps as bigint),
      positionState: b.state ? stateName(b.state.state) : null,
      advance: b.advance
        ? { principal: String(b.advance.principal as bigint), interest: String(b.advance.interest as bigint), lastAccrual: String(b.advance.last_accrual as bigint) }
        : null,
      payoutToken: problem ? null : ((payout?.token_account as string) ?? null),
      payoutProblem: problem,
    };
  }
  return (
    <>
      <h2>
        Facility #{String(c.facility_id as bigint)} <Addr a={f.address} />
      </h2>
      <div className="grid">
        <Stat k="collateral" v={<Addr a={c.asset_mint as string} />} />
        <Stat k="interest (APR)" v={pct(params.rate_bps)} />
        <Stat k="advance rate" v={pct(params.advance_rate_bps)} />
        <Stat k="maintenance LTV" v={pct(params.maintenance_ltv_bps)} />
        <Stat k="fund: funded / lent" v={f.fund ? `${fmt6(f.fund.total_funded as bigint, 2)} / ${fmt6(f.fund.principal_outstanding as bigint, 2)}` : "not open"} />
      </div>
      {position && (
        <p className="small">
          Your position: <b>{stateName(position.state)}</b> · collateral {fmt6(position.qty as bigint)} · advanced {fmt6(position.advanced as bigint)} USDC ·
          opened {utc(Number(position.opened_at as bigint))}
          {actions?.advance && (
            <>
              {" "}
              · owed {fmt6(BigInt(actions.advance.principal) + BigInt(actions.advance.interest))} USDC at the last accrual
            </>
          )}
        </p>
      )}
      {actions ? <FacilityActions {...actions} /> : !wallet && <p className="small muted">Look up a wallet to see its position and act.</p>}
    </>
  );
}

export default async function FacilityPage({ searchParams }: { searchParams: Promise<{ wallet?: string }> }) {
  await connection();
  const q = await searchParams;
  const wallet = q.wallet && isAddress(q.wallet) ? address(q.wallet) : null;
  let s: Awaited<ReturnType<typeof loadFacilities>> | null = null;
  let failure: string | null = null;
  let now = 0;
  try {
    [s, now] = await Promise.all([loadFacilities(), chainNow()]);
  } catch (e) {
    failure = (e as Error).message;
  }
  return (
    <main className="wrap">
      <Banner />
      <h1>Liquidity facility</h1>
      <p className="muted small">
        liquidity-facility (LF-P1..P6): pledge collateral (NPCS, or a pledgeable queue claim) and take a USDC advance at most the
        curator rate × the collateral at <code>MARK_PROTECTIVE</code>; top up to cure a breach; repay through payment-rails
        (interest first); release the collateral, with its seasoning lots, once the advance is closed.
      </p>
      {failure && <ErrorPanel title="RPC error">{failure}</ErrorPanel>}
      {s && !s.deployed && (
        <ErrorPanel title="liquidity-facility is not deployed on this cluster">
          Program <Addr a={PROGRAMS.facility} /> has no account here, so there is nothing to pledge into. On devnet it goes up after
          payment-rails upgrade #3 (see DEPLOYMENTS.md); the builders behind this page are pinned to its IDL by the app tests.
        </ErrorPanel>
      )}
      {s?.deployed && (
        <>
          <AutoRefresh seconds={20} />
          <WalletLookup current={wallet} />
          {s.facilities.length === 0 ? (
            <p className="muted small">No facility is open: <code>open_facility</code> (the program&apos;s upgrade authority) opens one.</p>
          ) : (
            s.facilities.map((f) => <Facility key={f.address} f={f} wallet={wallet} now={now} />)
          )}
        </>
      )}
    </main>
  );
}
