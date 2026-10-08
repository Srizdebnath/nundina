/**
 * treasury-agent (SRS 08 §6): runbook-driven, not autonomous. It holds no
 * key that moves money; deposits and withdrawals stay manual per
 * `token/runbook.md` § Treasury. What it does is the weekly reconciliation:
 * does the USDC behind NPCS equal the supply (doc 09 §3 I1)?
 *
 *   status    — read-only: supply, USDC held, I1 verdict, the treasury's
 *               lifetime totals and pause flag (program mode);
 *   reconcile — sends npcs-treasury's permissionless `reconcile`, which
 *               evaluates I1 on chain and emits `Reconciled { backed }` —
 *               an on-chain, timestamped record of the check.
 *
 * Two modes, as for the NAV: `treasury:<PDA>` (the I1 program, #148) or
 * `sleeve:<USDC account>` for the manual backup sleeve (devnet today).
 */
import { AccountRole, address, getAddressDecoder, type Address, type KeyPairSigner } from "@solana/kit";
import { fromBase64, type Struct } from "./codec.ts";
import { mintSupply, sendAndPoll, tokenAmount } from "./nav-publisher.ts";
import { PROGRAMS, programId } from "./programs.ts";
import type { R } from "./rpc.ts";

export interface TreasuryReport {
  mode: "treasury" | "sleeve";
  assetMint: Address;
  usdcAccount: Address;
  supplyBase: bigint;
  usdcMicros: bigint;
  /** NPCS base units per USDC micro (10^(asset_dec − usdc_dec)). */
  scale: bigint;
  backed: boolean;
  slot: bigint;
  treasury: Struct | null;
}

const decimals = (mint: Uint8Array) => mint[44]!;

export async function treasuryStatus(rpc: R, o: { source: string; mint?: Address }): Promise<TreasuryReport> {
  const [kind, acct] = o.source.split(":") as [string, string];
  let treasury: Struct | null = null;
  let assetMint: Address;
  let usdcAccount: Address;
  if (kind === "treasury") {
    const t = await rpc.getAccountInfo(address(acct), { encoding: "base64" }).send();
    if (!t.value) throw new Error(`treasury ${acct} not found`);
    treasury = PROGRAMS["npcs-treasury"].decodeAccount("Treasury", fromBase64(t.value.data[0]));
    assetMint = treasury.asset_mint as Address;
    usdcAccount = treasury.vault as Address;
  } else if (kind === "sleeve") {
    if (!o.mint) throw new Error("sleeve mode needs --mint");
    assetMint = o.mint;
    usdcAccount = address(acct);
  } else {
    throw new Error("--source must be treasury:<PDA> or sleeve:<USDC account>");
  }
  const { context, value } = await rpc.getMultipleAccounts([assetMint, usdcAccount], { encoding: "base64", commitment: "confirmed" }).send();
  if (!value[0] || !value[1]) throw new Error("mint or USDC account missing");
  const mint = fromBase64(value[0].data[0]);
  const usdc = fromBase64(value[1].data[0]);
  const usdcMintAddr = getAddressDecoder().decode(usdc.subarray(0, 32));
  const usdcMint = await rpc.getAccountInfo(usdcMintAddr, { encoding: "base64" }).send();
  if (!usdcMint.value) throw new Error("USDC mint missing");
  const assetDec = decimals(mint);
  const usdcDec = decimals(fromBase64(usdcMint.value.data[0]));
  if (assetDec < usdcDec) throw new Error("asset decimals below USDC decimals");
  const scale = 10n ** BigInt(assetDec - usdcDec);
  const supplyBase = mintSupply(mint);
  const usdcMicros = tokenAmount(usdc);
  return {
    mode: kind,
    assetMint,
    usdcAccount,
    supplyBase,
    usdcMicros,
    scale,
    backed: supplyBase === usdcMicros * scale,
    slot: context.slot,
    treasury,
  };
}

export function formatTreasury(r: TreasuryReport): string {
  const lines = [
    `mode            ${r.mode}`,
    `asset mint      ${r.assetMint}`,
    `USDC account    ${r.usdcAccount}`,
    `slot            ${r.slot}`,
    `NPCS supply     ${r.supplyBase} base units`,
    `USDC held       ${r.usdcMicros} µUSDC (× ${r.scale} = ${r.usdcMicros * r.scale} base units)`,
    `I1 supply == USDC × scale: ${r.backed ? "BACKED" : "NOT BACKED"}`,
  ];
  if (r.treasury) {
    lines.push(
      `deposited total ${r.treasury.total_deposited} µUSDC`,
      `redeemed total  ${r.treasury.total_redeemed} µUSDC`,
      `paused          ${r.treasury.paused}`,
    );
  }
  return lines.join("\n");
}

/** npcs-treasury `reconcile` (permissionless): I1 evaluated and evented on chain. */
export async function reconcileOnChain(rpc: R, payer: KeyPairSigner, treasuryPda: Address): Promise<{ signature: string; backed: boolean }> {
  const status = await treasuryStatus(rpc, { source: `treasury:${treasuryPda}` });
  const t = status.treasury!;
  const disc = PROGRAMS["npcs-treasury"].idl.instructions.find((i) => i.name === "reconcile")!.discriminator;
  const ix = {
    programAddress: programId("npcs-treasury"),
    accounts: [
      { address: treasuryPda, role: AccountRole.READONLY },
      { address: t.vault as Address, role: AccountRole.READONLY },
      { address: t.asset_mint as Address, role: AccountRole.READONLY },
      { address: t.usdc_mint as Address, role: AccountRole.READONLY },
    ],
    data: Uint8Array.from(disc),
  };
  const signature = await sendAndPoll(rpc, payer, [ix]);
  const tx = await rpc
    .getTransaction(signature as never, { encoding: "json", maxSupportedTransactionVersion: 0, commitment: "confirmed" })
    .send();
  const ev = PROGRAMS["npcs-treasury"].eventsFromLogs(tx?.meta?.logMessages ?? []).find((e) => e.name === "Reconciled");
  if (!ev) throw new Error(`no Reconciled event in ${signature}`);
  return { signature, backed: ev.data.backed as boolean };
}
