/**
 * Clone plan for the NPCS DvP rehearsal (`rehearse-npcs.sh`): which devnet
 * accounts a local validator must clone so `npcs-dvp.ts` runs against real
 * devnet state, plus the only three things that differ, written as files:
 *
 *   rails.so    — the approved Squads upgrade's buffer ELF (FqAJ…): the
 *                 exact build devnet will run once proposal #2 executes;
 *   lots.json   — holder H's credit-gate lot ledger with the seasoning
 *                 expiry moved into the past (on devnet it passes on
 *                 2026-10-11; nothing else in the ledger changes:
 *                 free_qty stays as on devnet, and npcs-dvp.ts runs
 *                 `merge_expired` to fold the expired lots into FREE,
 *                 exactly as it will on devnet);
 *   usdc.json   — the rails operator's Circle-USDC account with 5 USDC
 *                 (the devnet faucet is a captcha).
 *
 *   npx tsx scripts/devnet/npcs-clone.ts <outdir>   → <outdir>/plan.json
 */
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { address, getAddressEncoder, getProgramDerivedAddress, type Address } from "@solana/kit";
import { ownerTokenAccount, rawAccount } from "../../src/chain.ts";
import { loadKeypair, makeCtx } from "../../src/ctx.ts";
import { CREDIT_GATE } from "../../src/seasoning.ts";
import { findAssociatedTokenPda } from "@solana-program/token";
import { readDeployment } from "./common.ts";
import { NPCS, TOKEN_ACL, UPGRADE_BUFFER } from "./npcs-dvp.ts";

const SAS = address("22zoJMtdu4tQc2PzL74ZUT7FrwgB1Udec8DdW4yw4BdG");
const SAS_RECORD = {
  credential: address("7H1P4RLB9VhsrSHwqtxJJsMC5wMtF9T7HUN6EUovhfxV"),
  schemas: [
    address("6ooYMTCt89VLStUxPPWdTq19Ee9E9TaQCnJPxt3abRUL"),
    address("FVxSDQnri8zSREZTXjdgdXc2D8KPyNSFWFsUjG3BuWaU"),
    address("tdbagMb6dFGBTF83onfstpoeJqBMvHvsgV6m1VQniHs"),
  ],
};

const enc = getAddressEncoder();
const pda = async (program: Address, ...seeds: (string | Address | Uint8Array)[]) =>
  (
    await getProgramDerivedAddress({
      programAddress: program,
      seeds: seeds.map((s) => (s instanceof Uint8Array ? s : s.length >= 32 ? enc.encode(s as Address) : new TextEncoder().encode(s))),
    })
  )[0];

async function main() {
  const out = process.argv[2];
  if (!out) throw new Error("usage: npcs-clone.ts <outdir>");
  mkdirSync(out, { recursive: true });
  const rpcUrl = process.env.RPC_URL ?? "https://api.devnet.solana.com";
  const ctx = await makeCtx({ rpcUrl, payer: await loadKeypair(process.env.AUTHORITY ?? "~/.config/solana/turbine-devnet.json"), quiet: true });
  const d = readDeployment(rpcUrl);
  const H = (await loadKeypair(process.env.HOLDER ?? "~/.config/nundina/npcs-devnet/holder-h.json")).address;
  const operator = address(d.authority);

  const hLink = await pda(CREDIT_GATE, "holder", NPCS, H);
  const link = await rawAccount(ctx.rpc, hLink);
  if (!link) throw new Error("H has no holder link on devnet");
  const investorId = link.data.subarray(136, 152); // HolderLink: disc, mint, owner, 2 attestations, investor_id
  const hNpcs = await ownerTokenAccount(ctx.rpc, H, NPCS);
  const [opUsdc] = await findAssociatedTokenPda({ owner: operator, mint: address(d.usdcMint), tokenProgram: address(d.usdcTokenProgram) });
  if (!hNpcs) throw new Error("H's NPCS account is missing on devnet");
  if (!(await rawAccount(ctx.rpc, opUsdc))) throw new Error(`the operator's USDC ATA ${opUsdc} is missing on devnet`);

  const clone: Address[] = [
    NPCS,
    await pda(TOKEN_ACL, "MINT_CONFIG", NPCS),
    await pda(CREDIT_GATE, "gate_root"),
    await pda(CREDIT_GATE, "config", NPCS),
    await pda(CREDIT_GATE, "policy", NPCS),
    await pda(CREDIT_GATE, "blocklist", NPCS),
    await pda(CREDIT_GATE, "thaw_extra_account_metas", NPCS),
    await pda(CREDIT_GATE, "freeze_extra_account_metas", NPCS),
    hLink,
    await pda(CREDIT_GATE, "investor", NPCS, new Uint8Array(investorId)),
    hNpcs,
    SAS_RECORD.credential,
    ...SAS_RECORD.schemas,
    ...(await Promise.all(SAS_RECORD.schemas.slice(0, 2).map((s) => pda(SAS, "attestation", SAS_RECORD.credential, s, H)))),
    address(d.config),
    address(d.usdcMint),
    address(d.feeReceiver),
    address(d.reserve),
    operator,
    H,
  ].filter((a) => a !== opUsdc);

  // Patched 1: H's lots, every lot's restriction_expiry → an hour ago.
  const lotsA = await pda(CREDIT_GATE, "lots", NPCS, H);
  const lots = (await rawAccount(ctx.rpc, lotsA))!;
  const raw = await ctx.rpc.getAccountInfo(lotsA, { encoding: "base64" }).send();
  const data = new Uint8Array(lots.data);
  const v = new DataView(data.buffer);
  const past = BigInt(Math.floor(Date.now() / 1000) - 3600);
  for (let i = 0; i < Math.min(data[84]!, 16); i++) v.setBigInt64(85 + 31 * i + 20, past, true);
  const acct = (a: Address, bytes: Uint8Array, lamports: bigint, owner: Address) => ({
    pubkey: a,
    account: { lamports: Number(lamports), data: [Buffer.from(bytes).toString("base64"), "base64"], owner, executable: false, rentEpoch: 0, space: bytes.length },
  });
  writeFileSync(join(out, "lots.json"), JSON.stringify(acct(lotsA, data, raw.value!.lamports, CREDIT_GATE)));

  // Patched 2: the operator's USDC account holds 5 USDC.
  const u = await ctx.rpc.getAccountInfo(opUsdc, { encoding: "base64" }).send();
  const ud = new Uint8Array(Buffer.from(u.value!.data[0], "base64"));
  new DataView(ud.buffer).setBigUint64(64, 5_000_000n, true);
  writeFileSync(join(out, "usdc.json"), JSON.stringify(acct(opUsdc, ud, u.value!.lamports, u.value!.owner)));

  // The upgrade's ELF: a Buffer account is tag(4) | Option<authority>(33) | program bytes.
  const b = await ctx.rpc.getAccountInfo(UPGRADE_BUFFER, { encoding: "base64" }).send();
  if (!b.value) throw new Error("upgrade buffer not found: proposal #2 already executed? rehearse against devnet's program instead");
  const elf = Buffer.from(b.value.data[0], "base64").subarray(37);
  writeFileSync(join(out, "rails.so"), elf);

  const plan = { clone, patched: { lots: lotsA, usdc: opUsdc }, programs: { gate: CREDIT_GATE, tokenAcl: TOKEN_ACL, sas: SAS, rails: d.programId } };
  writeFileSync(join(out, "plan.json"), JSON.stringify(plan, null, 2));
  console.log(`${clone.length} accounts to clone, 2 patched, rails.so ${elf.length} bytes`);
}

main().catch((e) => {
  console.error("✗", e instanceof Error ? e.message : e);
  process.exit(1);
});
