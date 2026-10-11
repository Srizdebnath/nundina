import React from "react";
import type { Address } from "@solana/kit";
import { Addr, Banner, ErrorPanel, Stat } from "@/components/app/ui";
import { AutoRefresh } from "@/components/app/client";
import { TrancheActions } from "@/components/app/FlowActions";
import { loadPools, mintInfos } from "@/lib/flows-read";
import { PROGRAMS } from "@/lib/programs";
import { fmt6 } from "@/lib/session";
import type { Struct } from "@/lib/borsh";
import { connection } from "next/server";


const pct = (bps: unknown) => `${(Number(bps as bigint) / 100).toFixed(2)}%`;
const stateName = (v: unknown) => (typeof v === "string" ? v : (Object.keys(v as Struct)[0] ?? "?"));

async function Pool({ address: a, pool: p }: { address: Address; pool: Struct }) {
  const params = p.params as Struct;
  const [collateral, senior] = await mintInfos([p.collateral_mint as Address, p.senior_mint as Address]);
  const state = stateName(p.state);
  return (
    <>
      <h2>
        Pool #{p.pool_id as number} <Addr a={a} /> · {state}
      </h2>
      <div className="grid">
        <Stat k="collateral" v={`${fmt6(p.collateral_qty as bigint)}`} />
        <Stat k="senior principal" v={fmt6(p.senior_principal as bigint, 2)} />
        <Stat k="junior principal" v={fmt6(p.junior_principal as bigint, 2)} />
        <Stat k="attachment (junior share)" v={pct(params.attachment_bps)} />
        <Stat k="coupons senior / junior" v={`${pct(params.senior_coupon_bps)} / ${pct(params.junior_coupon_bps)}`} />
        <Stat k="realised loss" v={fmt6(p.realised_loss as bigint, 2)} cls={(p.realised_loss as bigint) > 0n ? "bad" : ""} />
      </div>
      {collateral && senior ? (
        <TrancheActions
          pool={a}
          feed={p.feed as string}
          seniorMint={p.senior_mint as string}
          juniorMint={p.junior_mint as string}
          collateralVault={p.collateral_vault as string}
          collateralMint={collateral.address}
          collateralDecimals={collateral.decimals}
          collateralTokenProgram={collateral.program}
          trancheTokenProgram={senior.program}
          attachmentBps={String(params.attachment_bps as bigint)}
          open={state === "Warehousing" || state === "Active"}
        />
      ) : (
        <p className="small bad">The pool&apos;s mints are not readable on this cluster.</p>
      )}
    </>
  );
}

export default async function TranchesPage() {
  await connection();
  let s: Awaited<ReturnType<typeof loadPools>> | null = null;
  let failure: string | null = null;
  try {
    s = await loadPools();
  } catch (e) {
    failure = (e as Error).message;
  }
  return (
    <main className="wrap">
      <Banner />
      <h1>Tranches</h1>
      <p className="muted small">
        tranche-engine (TE-P1..P7): deposit collateral into a pool at <code>MARK_PROTECTIVE</code> (Fresh mark only). One deposit
        mints both tranches: junior = value × the attachment (frozen for the lock-up), senior = the rest. Positions are on the
        portfolio page.
      </p>
      {failure && <ErrorPanel title="RPC error">{failure}</ErrorPanel>}
      {s && !s.deployed && (
        <ErrorPanel title="tranche-engine is not deployed on this cluster">
          Program <Addr a={PROGRAMS.tranche} /> has no account here.
        </ErrorPanel>
      )}
      {s?.deployed && s.pools.length === 0 && (
        <p className="muted small">
          No pool yet. <code>create_pool</code> (the program&apos;s upgrade authority) opens one per pool id; on devnet that waits
          for the rails <code>ROLE_COUPON</code> grant (DEPLOYMENTS.md).
        </p>
      )}
      {s?.deployed && s.pools.length > 0 && (
        <>
          <AutoRefresh seconds={30} />
          {s.pools.map((p) => (
            <Pool key={p.address} {...p} />
          ))}
        </>
      )}
    </main>
  );
}
