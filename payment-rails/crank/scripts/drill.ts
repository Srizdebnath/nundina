/**
 * Kill-the-crank drill (PHASES.md R4 exit criterion), against a real
 * solana-test-validator running the built program (see run-drill.sh).
 *
 *  1. Two sellers, two revealed buyers, one bidder who never reveals.
 *  2. The crank settles fill #0 and dies mid-plan.
 *  3. It restarts: #0 is skipped (receipt exists), #1 settles, it dies for
 *     good — `finish_session` is never called.
 *  4. Before the deadline a participant can already refund the non-revealer
 *     (bond → reserve); revealed escrows are correctly still locked.
 *  5. After the deadline that participant — with their own wallet, no crank
 *     key — refunds everyone. A second run finds nothing left.
 *  6. Reconciliation is clean, balances are exact and conserved, and the
 *     evidence export's SHA-256 re-verifies.
 */
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import {
  airdropFactory,
  getAddressEncoder,
  generateKeyPairSigner,
  getProgramDerivedAddress,
  address,
  lamports,
  type Address,
  type KeyPairSigner,
} from "@solana/kit";
import { getCreateAccountInstruction } from "@solana-program/system";
import {
  getInitializeAccountInstruction,
  getInitializeMintInstruction,
  getMintSize,
  getMintToInstruction,
  getTokenSize,
  TOKEN_PROGRAM_ADDRESS,
} from "@solana-program/token";
import { clockNow, exists, tokenBalance } from "../src/chain.ts";
import { loadSession, refundAll, settlePlan, type FillPlan } from "../src/crank.ts";
import { loadKeypair, makeCtx } from "../src/ctx.ts";
import { exportEvidence } from "../src/evidence.ts";
import { ix, PROGRAM_ID } from "../src/idl.ts";
import { formatReport, reconcileFacility, reconcileSession } from "../src/ledger.ts";
import {
  advancePda, assetVaultPda, configPda, destinationPda, escrowPda, facilityPda, facilityVaultPda, sessionPda, vaultPda,
} from "../src/pda.ts";
import { send, type Ctx } from "../src/send.ts";

const USDC = 1_000_000n;
const UNIT = 1_000_000n;
const PRICE = 98_123_456n;
const BOND = 5n * USDC;
const LOADER_V3 = address("BPFLoaderUpgradeab1e11111111111111111111111");

let pass = 0;
let fail = 0;
function check(ok: boolean, label: string): void {
  ok ? pass++ : fail++;
  console.log(`${ok ? "PASS" : "FAIL"}  ${label}`);
}
async function must(ctx: Ctx, label: string, instructions: Parameters<typeof send>[2]): Promise<void> {
  const r = await send(ctx, label, instructions);
  if (r.status !== "sent") throw new Error(`${label}: ${r.name ?? r.message}`);
}
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
async function waitUntil(ctx: Ctx, t: bigint): Promise<void> {
  while ((await clockNow(ctx.rpc)) < t) await sleep(400);
}

/** fee/reserve split mirror (buyer pays ceil, fee/reserve floored). */
function split(qty: bigint, price: bigint) {
  const n = qty * price;
  const gross = n / UNIT + (n % UNIT ? 1n : 0n);
  const fee = (gross * 10n) / 10_000n;
  const res = (gross * 5n) / 10_000n;
  return { gross, fee, res, seller: gross - fee - res };
}

async function main(): Promise<void> {
  const authority = await loadKeypair(process.env.AUTHORITY!);
  const ops = await makeCtx({ payer: authority, quiet: !process.env.VERBOSE });
  const airdrop = airdropFactory({ rpc: ops.rpc, rpcSubscriptions: ops.rpcSubscriptions });
  const fund = (a: Address) => airdrop({ recipientAddress: a, lamports: lamports(5_000_000_000n), commitment: "confirmed" });
  await fund(authority.address);

  // ── tokens (SPL Token for both mints; Token-2022 is covered in LiteSVM) ──
  const rent = async (size: number) => ops.rpc.getMinimumBalanceForRentExemption(BigInt(size)).send();
  async function newMint(): Promise<Address> {
    const mint = await generateKeyPairSigner();
    await must(ops, "create mint", [
      getCreateAccountInstruction({ payer: authority, newAccount: mint, lamports: await rent(getMintSize()), space: getMintSize(), programAddress: TOKEN_PROGRAM_ADDRESS }),
      getInitializeMintInstruction({ mint: mint.address, decimals: 6, mintAuthority: authority.address }),
    ]);
    return mint.address;
  }
  async function newAccount(mint: Address, owner: Address, amount: bigint): Promise<Address> {
    const acct = await generateKeyPairSigner();
    await must(ops, "create token account", [
      getCreateAccountInstruction({ payer: authority, newAccount: acct, lamports: await rent(getTokenSize()), space: getTokenSize(), programAddress: TOKEN_PROGRAM_ADDRESS }),
      getInitializeAccountInstruction({ account: acct.address, mint, owner }),
      ...(amount > 0n ? [getMintToInstruction({ mint, token: acct.address, mintAuthority: authority, amount })] : []),
    ]);
    return acct.address;
  }
  const usdc = await newMint();
  const asset = await newMint();
  const fee = await newAccount(usdc, authority.address, 0n);
  const reserve = await newAccount(usdc, authority.address, 0n);

  interface Party { key: KeyPairSigner; usdc: Address; asset: Address }
  async function party(u: bigint, a: bigint): Promise<Party> {
    const key = await generateKeyPairSigner();
    await fund(key.address);
    return { key, usdc: await newAccount(usdc, key.address, u), asset: await newAccount(asset, key.address, a) };
  }

  // ── rails setup ──────────────────────────────────────────────────────────
  const config = await configPda();
  const [programData] = await getProgramDerivedAddress({ programAddress: LOADER_V3, seeds: [getAddressEncoder().encode(PROGRAM_ID)] });
  await must(ops, "initialize_config", [
    ix("initialize_config", {
      config, usdc_mint: usdc, fee_receiver: fee, reserve, authority, program: PROGRAM_ID,
      program_data: programData, token_program: TOKEN_PROGRAM_ADDRESS,
    }),
  ]);
  await must(ops, "set_fees", [ix("set_fees", { config, authority }, { fee_bps: 10, reserve_bps: 5 })]);

  const [A, B] = [await party(BOND, UNIT), await party(BOND, 2n * UNIT)];
  const [X, Y, Z] = [await party(300n * USDC + BOND, 0n), await party(150n * USDC + BOND, 0n), await party(50n * USDC + BOND, 0n)];
  const everyone = [A, B, X, Y, Z];

  const t0 = await clockNow(ops.rpc);
  const [commitEnd, revealEnd, deadline] = [t0 + 12n, t0 + 22n, t0 + 50n];
  const session = await sessionPda(authority.address, 1n);
  await must(ops, "open_session", [
    ix("open_session", { config, session, asset_mint: asset, operator: authority }, {
      session_id: 1n,
      params: { commit_end: commitEnd, reveal_end: revealEnd, settle_deadline: deadline, bond_amount: BOND, max_cash: 1_000n * USDC, rofr_authority: address("11111111111111111111111111111111") },
    }),
  ]);

  const usdcAccounts = [fee, reserve, ...everyone.map((p) => p.usdc)];
  const assetAccounts = everyone.map((p) => p.asset);
  const sum = async (xs: Address[]) => (await Promise.all(xs.map((x) => tokenBalance(ops.rpc, x)))).reduce<bigint>((a, b) => a + (b ?? 0n), 0n);
  const [usdc0, asset0] = [await sum(usdcAccounts), await sum(assetAccounts)];

  for (const [p, qty] of [[A, UNIT], [B, 2n * UNIT]] as const) {
    const escrow = await escrowPda(session, p.key.address);
    const assetVault = await assetVaultPda(escrow);
    await must(ops, "deposit_ask", [
      // Unfrozen asset: open the vault and deposit in one transaction.
      ix("open_asset_vault", {
        config, session, escrow, asset_vault: assetVault, asset_mint: asset, owner: p.key,
        asset_token_program: TOKEN_PROGRAM_ADDRESS,
      }),
      ix("deposit_ask", {
        config, session, escrow, vault: await vaultPda(escrow), asset_vault: assetVault,
        usdc_mint: usdc, asset_mint: asset, owner_usdc: p.usdc, owner_asset: p.asset, owner: p.key,
        token_program: TOKEN_PROGRAM_ADDRESS, asset_token_program: TOKEN_PROGRAM_ADDRESS,
      }, { qty }),
    ]);
  }
  for (const [p, cash] of [[X, 300n * USDC], [Y, 150n * USDC], [Z, 50n * USDC]] as const) {
    const escrow = await escrowPda(session, p.key.address);
    await must(ops, "deposit", [
      ix("deposit", {
        config, session, escrow, vault: await vaultPda(escrow), usdc_mint: usdc, owner_token: p.usdc,
        owner: p.key, token_program: TOKEN_PROGRAM_ADDRESS,
      }, { cash }),
    ]);
  }
  await waitUntil(ops, commitEnd);
  for (const p of [A, B, X, Y]) {
    await must(ops, "mark_revealed", [ix("mark_revealed", { session, escrow: await escrowPda(session, p.key.address), operator: authority })]);
  }
  await waitUntil(ops, revealEnd);
  console.log("setup done: 2 asks, 3 bids (Z never revealed), settlement window open\n");

  // ── crank: dies after one fill, restarts, dies again ─────────────────────
  const plan: FillPlan[] = [
    { index: 0, buyer: X.key.address, seller: A.key.address, qty: UNIT, price: PRICE },
    { index: 1, buyer: X.key.address, seller: B.key.address, qty: (3n * UNIT) / 2n, price: PRICE },
    { index: 2, buyer: Y.key.address, seller: B.key.address, qty: UNIT / 2n, price: PRICE },
  ];
  const run1 = await settlePlan(ops, session, authority, plan, { maxSends: 1 });
  check(run1.map((o) => o.result).join() === "sent,skipped,skipped", `crank run 1 settles #0 then dies: ${run1.map((o) => o.result)}`);
  const run2 = await settlePlan(ops, session, authority, plan, { maxSends: 1 });
  check(run2.map((o) => o.result).join() === "already-done,sent,skipped", `restarted crank resumes idempotently: ${run2.map((o) => o.result)}`);
  check((await loadSession(ops.rpc, session)).fills_settled === 2, "exactly 2 fills on-chain (no double settlement)");
  // Mid-session: live escrows, non-zero totals — the ledger must still balance.
  const mid = await reconcileSession(ops.rpc, session);
  const live = mid.checks.find((c) => c.name.startsWith("open escrows"))!;
  check(mid.ok && live.actual === 5n, `mid-session reconciliation clean with ${live.actual} live escrows`);
  console.log("crank is now dead for good — finish_session is never called\n");

  // ── any participant, own wallet, no crank ────────────────────────────────
  const participant = await makeCtx({ payer: Y.key, quiet: !process.env.VERBOSE });
  const early = await refundAll(participant, session);
  const byResult = (os: typeof early, r: string) => os.filter((o) => o.result === r).length;
  check(byResult(early, "sent") === 1 && byResult(early, "not-yet") === 4, `before deadline: only non-revealer Z refundable (sent ${byResult(early, "sent")}, locked ${byResult(early, "not-yet")})`);
  check((await tokenBalance(ops.rpc, Z.usdc)) === 50n * USDC && (await tokenBalance(ops.rpc, reserve))! >= BOND, "Z got cash back, bond went to reserve");

  await waitUntil(ops, deadline);
  const late = await refundAll(participant, session);
  check(byResult(late, "sent") === 4 && byResult(late, "failed") === 0, `after deadline: participant refunds all 4 remaining escrows (sent ${byResult(late, "sent")})`);
  const again = await refundAll(participant, session);
  check(again.length === 0, "second refund run finds nothing left (idempotent)");
  for (const p of everyone) check(!(await exists(ops.rpc, await escrowPda(session, p.key.address))), `escrow of ${p.key.address.slice(0, 6)}… closed`);

  // ── proof ────────────────────────────────────────────────────────────────
  const report = await reconcileSession(ops.rpc, session);
  console.log(formatReport(report));
  check(report.ok, "reconciliation clean: escrowed balances == open obligations (all zero)");

  const [s0, s1] = [split(UNIT, PRICE), split((3n * UNIT) / 2n, PRICE)];
  const bal = (a: Address) => tokenBalance(ops.rpc, a).then((b) => b ?? 0n);
  check((await bal(X.usdc)) === 300n * USDC + BOND - s0.gross - s1.gross, "X paid exactly the two settled fills, rest + bond refunded");
  check((await bal(X.asset)) === (5n * UNIT) / 2n, "X holds 2.5 asset");
  check((await bal(Y.usdc)) === 150n * USDC + BOND && (await bal(Y.asset)) === 0n, "Y (fill never settled) fully refunded");
  check((await bal(A.usdc)) === BOND + s0.seller && (await bal(A.asset)) === 0n, "A: proceeds + bond");
  check((await bal(B.usdc)) === BOND + s1.seller && (await bal(B.asset)) === UNIT / 2n, "B: proceeds + bond + unsold 0.5 back");
  check((await bal(Z.usdc)) === 50n * USDC, "Z: cash back, bond forfeited");
  check((await bal(fee)) === s0.fee + s1.fee && (await bal(reserve)) === s0.res + s1.res + BOND, "fee and reserve exact");
  check((await sum(usdcAccounts)) === usdc0 && (await sum(assetAccounts)) === asset0, "USDC and asset conserved end to end");

  // ── facility ledger (R3 cash, R4 reconciliation) ─────────────────────────
  const facility = await facilityPda(authority.address, 1n);
  const facilityVault = await facilityVaultPda(facility);
  await must(ops, "open_facility", [
    ix("open_facility", { config, facility, vault: facilityVault, usdc_mint: usdc, operator: authority, token_program: TOKEN_PROGRAM_ADDRESS }, { facility_id: 1n, rate_bps: 1_200n }),
  ]);
  const treasury = await party(10_000n * USDC, 0n);
  await must(ops, "fund_facility", [
    ix("fund_facility", { facility, vault: facilityVault, usdc_mint: usdc, funder_token: treasury.usdc, funder: treasury.key, token_program: TOKEN_PROGRAM_ADDRESS }, { amount: 10_000n * USDC }),
  ]);
  const far = (await clockNow(ops.rpc)) + 10n * 31_536_000n;
  const dest = await destinationPda(X.key.address);
  await must(ops, "register_destination", [
    ix("register_destination", { config, destination: dest, owner: X.key.address, token_account: X.usdc, registrar: authority }, { attestation_ref_hash: new Uint8Array(32).fill(0xab), expires_at: far }),
  ]);
  await must(ops, "advance_funds", [
    ix("advance_funds", {
      config, facility, advance: await advancePda(facility, X.key.address), vault: facilityVault, borrower: X.key.address,
      destination: dest, destination_token: X.usdc, usdc_mint: usdc, curator: authority, token_program: TOKEN_PROGRAM_ADDRESS,
    }, { amount: 1_000n * USDC, collateral_ref_hash: new Uint8Array(32).fill(0xc1) }),
  ]);
  const fr = await reconcileFacility(ops.rpc, facility);
  console.log(formatReport(fr));
  check(fr.ok, "facility reconciliation clean: vault = funded − outstanding + interest");

  writeFileSync(process.env.DRILL_OUT ?? "/dev/null", JSON.stringify({ session, facility }) + "\n");
  const dir = process.env.EVIDENCE_DIR!;
  const manifest = await exportEvidence(ops.rpc, dir);
  const body = readFileSync(join(dir, "events.jsonl"));
  check(createHash("sha256").update(body).digest("hex") === manifest.sha256, `evidence manifest SHA-256 re-verifies (${manifest.events} events, ${manifest.transactions} txs)`);
  check(manifest.byEvent.FillSettled === 2, `evidence: 2 FillSettled (got ${manifest.byEvent.FillSettled})`);
  const lines = body.toString().trim().split("\n").map((l) => JSON.parse(l));
  const refunded = lines.filter((l) => l.event === "EscrowEvent" && l.data.action === "Refunded").length;
  const forfeited = lines.filter((l) => l.event === "EscrowEvent" && l.data.action === "BondForfeited").length;
  check(refunded === 5 && forfeited === 1, `evidence: 5 Refunded + 1 BondForfeited (got ${refunded} + ${forfeited})`);
  console.log(`evidence written to ${dir} (manifest ${manifest.sha256.slice(0, 16)}…)`);

  console.log(`\n${pass} passed, ${fail} failed`);
  process.exit(fail ? 1 : 0);
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
