import React from "react";
import type { Address } from "@solana/kit";
import { Addr, Banner, ErrorPanel, Stat } from "@/components/app/ui";
import { AutoRefresh } from "@/components/app/client";
import { TreasuryActions } from "@/components/app/FlowActions";
import { loadTreasury, mintInfos, type TreasuryState } from "@/lib/flows-read";
import { NPCS_MINT, PROGRAMS } from "@/lib/programs";
import { fmt6 } from "@/lib/session";
import { connection } from "next/server";


export default async function TreasuryPage() {
  await connection();
  let s: TreasuryState | null = null;
  let mints: Awaited<ReturnType<typeof mintInfos>> = [];
  let failure: string | null = null;
  try {
    s = await loadTreasury(NPCS_MINT);
    mints = s.treasury ? await mintInfos([NPCS_MINT, s.treasury.usdc_mint as Address]) : [];
  } catch (e) {
    failure = (e as Error).message;
  }
  const [asset, usdc] = mints;
  const t = s?.treasury;
  // I1: supply == vault × scale.
  const scale = asset && usdc ? 10n ** BigInt(asset.decimals - usdc.decimals) : null;
  const i1 = asset && scale !== null && s?.vaultAmount != null ? asset.supply === s.vaultAmount * scale : null;
  return (
    <main className="wrap">
      <Banner />
      <h1>NPCS treasury</h1>
      <p className="muted small">
        npcs-treasury (doc 09 I1): USDC in, the same value of NPCS minted; the treasury authority burns NPCS to take USDC out.
        Every mint and burn checks NPCS supply = USDC in the vault, before and after.
      </p>
      {failure && <ErrorPanel title="RPC error">{failure}</ErrorPanel>}
      {s && !s.deployed && (
        <ErrorPanel title="npcs-treasury is not deployed on this cluster">
          Program <Addr a={PROGRAMS.treasury} /> has no account here.
        </ErrorPanel>
      )}
      {s?.deployed && !t && (
        <ErrorPanel title="The treasury is not initialized for this asset">
          <Addr a={s.address} /> does not exist yet: the NPCS mint authority has to be handed to the treasury signer and{" "}
          <code>initialize</code> run (on devnet a Squads proposal, <code>ops/squads migrate.ts propose-treasury</code>; see
          DEPLOYMENTS.md). Until then NPCS is backed by the sleeve account, not by this program.
        </ErrorPanel>
      )}
      {t && (
        <>
          <AutoRefresh seconds={20} />
          <div className="grid">
            <Stat k="treasury" v={<Addr a={s!.address} />} />
            <Stat k="vault USDC" v={s!.vaultAmount !== null ? fmt6(s!.vaultAmount) : "—"} />
            <Stat k="NPCS supply" v={asset ? fmt6(asset.supply) : "—"} />
            <Stat k="I1 supply = vault" v={i1 === null ? "—" : i1 ? "holds" : "BROKEN"} cls={i1 === null ? "muted" : i1 ? "ok" : "bad"} />
            <Stat k="deposited / redeemed" v={`${fmt6(t.total_deposited as bigint, 2)} / ${fmt6(t.total_redeemed as bigint, 2)}`} />
            <Stat k="authority" v={<Addr a={t.authority as string} />} />
          </div>
          <h2>Deposit / redeem</h2>
          {asset && usdc ? (
            <TreasuryActions
              assetMint={asset.address}
              usdcMint={usdc.address}
              vault={t.vault as string}
              authority={t.authority as string}
              paused={t.paused as boolean}
              assetDecimals={asset.decimals}
              usdcDecimals={usdc.decimals}
              assetTokenProgram={asset.program}
              usdcTokenProgram={usdc.program}
            />
          ) : (
            <p className="small bad">The treasury&apos;s mints are not readable on this cluster.</p>
          )}
        </>
      )}
    </main>
  );
}
