/**
 * The headline demo on devnet (AC-3 / AC-4, #145, #172): one sealed-bid
 * auction-engine session over the live NPCS mint, end to end, every step a
 * real transaction:
 *
 *   0. payment-rails `grant_engine`: the auction operator PDA may open rails
 *      sessions (needs rails upgrade #3, #181);
 *   1. buyers X, Y attested (SAS), validated by credit-gate, NPCS accounts
 *      thawed through Token ACL; H's seasoned lots merged into FREE;
 *   2. `open_session` (venue authority) → rails `open_session` by CPI;
 *      H opens the escrow's NPCS vault, the Token ACL authority thaws it, H
 *      `commit_ask`s 1.0 NPCS sealed; X, Y `commit_bid` sealed;
 *   3. everyone `reveal`s; the crank's phase runner (#149) runs `clear`
 *      (uniform price against the reserve from the mark-engine NAV),
 *      `allocate`, `settle` each fill DvP (rails `settle_fill` → credit-gate
 *      `settle_check`), `finish` (the print into mark-engine);
 *   4. refunds of what did not fill, reconciliation, the signed session
 *      evidence bundle (`crank export --session`).
 *
 *   npx tsx scripts/devnet/auction-session.ts --preflight   # read-only: what still blocks it
 *   npx tsx scripts/devnet/auction-session.ts
 *
 * Env: RPC_URL, AUTHORITY (venue + rails authority, 9WH1), HOLDER (H),
 * ACL_AUTHORITY (Token ACL MintConfig authority), SAS_ATTESTER; COMMIT_SECS
 * (default 240) and REVEAL_SECS (default 150) size the windows.
 */
import { createHash, randomBytes } from "node:crypto";
import { existsSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import {
  AccountRole,
  address,
  createNoopSigner,
  generateKeyPairSigner,
  getAddressEncoder,
  getProgramDerivedAddress,
  type Address,
  type Instruction,
  type KeyPairSigner,
} from "@solana/kit";
import { clockNow, ownerTokenAccount, rawAccount, tokenBalance } from "../../src/chain.ts";
import { refundAll } from "../../src/crank.ts";
import { loadKeypair, makeCtx } from "../../src/ctx.ts";
import { ix as railsIx, loadIdl, makeCodec } from "../../src/idl.ts";
import { formatReport, reconcileSession } from "../../src/ledger.ts";
import { assetVaultPda, configPda, escrowPda, le, sessionPda, vaultPda } from "../../src/pda.ts";
import { auction, AUCTION, loadAuction, runPhases } from "../../src/phases.ts";
import { mergeExpiredIx } from "../../src/seasoning.ts";
import { findAssociatedTokenPda } from "@solana-program/token";
import { exportSessionEvidence } from "../../src/session-evidence.ts";
import { ensureAta, explorer, must, readDeployment, stateDir, transferSol, transferToken, waitUntil, writeJson } from "./common.ts";
import { aclThawIx, gateCli, lotsPda, NPCS, sellable, TOKEN_2022 } from "./npcs-dvp.ts";

/** Rails upgrade #3's buffer (ops/squads/deployment-devnet.json): closed once #3 executes. */
export const UPGRADE_3_BUFFER = address("FwMUAaQKT6sR6cHApjCRM1UHHfAibJ5feY4wCYV1y1GE");
const RAILS = address(loadIdl("payment_rails").address);
const mark = makeCodec(loadIdl("mark_engine"));
const SYSTEM = address("11111111111111111111111111111111");
const ROLE_SESSIONS = 1;

const U = 1_000_000n;
const BOND = U / 10n;
/** Sealed orders: H sells 1.0 at ≥ 0.96; X buys 0.6 at ≤ 0.99, Y 0.4 at ≤ 0.97 (NAV 1.00, reserve 0.95). */
const ASK = { limit: (96n * U) / 100n, qty: U };
const BIDS = { X: { limit: (99n * U) / 100n, qty: (6n * U) / 10n }, Y: { limit: (97n * U) / 100n, qty: (4n * U) / 10n } };
const MAX_DISCOUNT_BPS = 500n;
const SOL_EACH = 30_000_000n;
const home = (p: string) => p.replace(/^~(?=\/)/, homedir());

/** USDC for `qty` at `limit`, rounded up like the engine's `gross` (the escrow must cover it). */
export const gross = (qty: bigint, limit: bigint) => (qty * limit + U - 1n) / U;

/** auction_engine::commitment: sha256(side ‖ limit ‖ qty ‖ nonce ‖ owner), side Buy = 0, Sell = 1. */
export function commitment(side: "Buy" | "Sell", limit: bigint, qty: bigint, nonce: Uint8Array, owner: Address): Uint8Array {
  const b = Buffer.alloc(17);
  b[0] = side === "Buy" ? 0 : 1;
  b.writeBigUInt64LE(limit, 1);
  b.writeBigUInt64LE(qty, 9);
  return new Uint8Array(createHash("sha256").update(b).update(nonce).update(getAddressEncoder().encode(owner) as Uint8Array).digest());
}

const enc = getAddressEncoder();
const pdaOf = async (program: Address, ...seeds: (string | Address | Uint8Array)[]) =>
  (
    await getProgramDerivedAddress({
      programAddress: program,
      seeds: seeds.map((s) => (s instanceof Uint8Array ? s : s.length >= 32 ? enc.encode(s as Address) : new TextEncoder().encode(s))),
    })
  )[0];

/** A rails instruction's accounts, forwarded behind an engine instruction: the operator PDA is de-signed (the engine signs for it). */
function forwarded(inner: Instruction, operator: Address) {
  return (inner.accounts ?? []).map((m) =>
    m.address === operator ? { address: m.address, role: m.role === AccountRole.WRITABLE_SIGNER ? AccountRole.WRITABLE : AccountRole.READONLY } : m,
  );
}
const withRails = (outer: Instruction, extra: ReturnType<typeof forwarded>): Instruction => ({
  ...outer,
  accounts: [...(outer.accounts ?? []), ...extra],
});

interface Keys {
  authority: string;
  holder: string;
  aclAuthority: string;
  sasAttester: string;
}

async function preflight(rpcUrl: string, keys: Keys): Promise<string[]> {
  const ctx = await makeCtx({ rpcUrl, payer: await generateKeyPairSigner(), quiet: true });
  const blocks: string[] = [];
  for (const [what, p] of Object.entries(keys)) if (!existsSync(home(p))) blocks.push(`missing key: ${what} (${p})`);
  if (await rawAccount(ctx.rpc, UPGRADE_3_BUFFER)) {
    blocks.push(
      "devnet payment-rails predates GrantEngine: Squads upgrade #3 is approved, not executed " +
        "(ops/squads: CLUSTER=devnet npx tsx migrate.ts execute-upgrade GdWmRivxe1k76apQB7EgJ4yBq8Fi3Lw25E8V3XosPocn; timelock opens 2026-10-09 20:43 UTC)",
    );
  }
  const now = await clockNow(ctx.rpc);
  const feedA = await pdaOf(mark.programId, "feed", NPCS);
  const feedR = await rawAccount(ctx.rpc, feedA);
  if (!feedR) blocks.push(`no mark-engine feed for NPCS (${feedA})`);
  else {
    const f = mark.decodeAccount("Feed", feedR.data) as { suspended: boolean; computed_ts: bigint; params: { warn_after: bigint } };
    if (f.suspended) blocks.push("the NPCS feed is suspended: clear would fail the session");
    if (now - f.computed_ts >= f.params.warn_after) blocks.push("the NPCS NAV is older than warn_after: publish this week's NAV first (services `npm run svc -- nav`)");
  }
  const venueR = await rawAccount(ctx.rpc, await pdaOf(AUCTION, "venue"));
  if (!venueR) blocks.push("no auction-engine venue");
  if (existsSync(home(keys.authority)) && venueR) {
    const a = await loadKeypair(keys.authority);
    const v = auction.decodeAccount("Venue", venueR.data) as { authority: Address };
    if (v.authority !== a.address) blocks.push(`AUTHORITY ${a.address} is not the venue authority ${v.authority}`);
    const d = readDeployment(rpcUrl);
    // The run funds the buyers from the authority's associated USDC account (ensureAta), not any other.
    const [usdc] = await findAssociatedTokenPda({ owner: a.address, mint: address(d.usdcMint), tokenProgram: address(d.usdcTokenProgram) });
    const need = gross(BIDS.X.qty, BIDS.X.limit) + gross(BIDS.Y.qty, BIDS.Y.limit) + 3n * BOND;
    const have = (await tokenBalance(ctx.rpc, usdc)) ?? 0n;
    if (have < need) blocks.push(`authority holds ${have} µUSDC < ${need} to fund the buyers and bonds (https://faucet.circle.com)`);
    const sol = (await ctx.rpc.getBalance(a.address).send()).value;
    if (sol < 500_000_000n) blocks.push(`authority has ${sol} lamports < 0.5 SOL`);
  }
  if (existsSync(home(keys.holder))) {
    const H = await loadKeypair(keys.holder);
    const lots = await rawAccount(ctx.rpc, await lotsPda(H.address));
    if (!lots) blocks.push(`H ${H.address} has no lot ledger`);
    else {
      const s = sellable(lots.data, now);
      if (s.free + s.seasonedInLots < ASK.qty) {
        blocks.push(
          `H's NPCS is still seasoning (CG-P3): sellable ${s.free + s.seasonedInLots} < ${ASK.qty}` +
            (s.nextExpiry !== null ? `; sellable from ${new Date(Number(s.nextExpiry) * 1000).toISOString()}` : ""),
        );
      }
    }
  }
  return blocks;
}

async function main(): Promise<void> {
  const rpcUrl = process.env.RPC_URL ?? "https://api.devnet.solana.com";
  const keys: Keys = {
    authority: process.env.AUTHORITY ?? "~/.config/solana/turbine-devnet.json",
    holder: process.env.HOLDER ?? "~/.config/nundina/npcs-devnet/holder-h.json",
    aclAuthority: process.env.ACL_AUTHORITY ?? "~/.config/nundina/npcs-devnet/freeze-authority.json",
    sasAttester: process.env.SAS_ATTESTER ?? "~/.config/nundina/sas-devnet/attester.json",
  };
  const blocks = await preflight(rpcUrl, keys);
  if (process.argv.includes("--preflight") || blocks.length > 0) {
    console.log(blocks.length ? `NOT READY:\n  - ${blocks.join("\n  - ")}` : "READY: every precondition holds");
    process.exit(blocks.length ? 1 : 0);
  }

  const d = readDeployment(rpcUrl);
  const authority = await loadKeypair(keys.authority);
  const H = await loadKeypair(keys.holder);
  const acl = await loadKeypair(keys.aclAuthority);
  const ops = await makeCtx({ rpcUrl, payer: authority });
  const usdc = address(d.usdcMint);
  const usdcTp = address(d.usdcTokenProgram);
  const config = await configPda();
  const venue = await pdaOf(AUCTION, "venue");
  const operator = await pdaOf(AUCTION, "rails_operator");
  const feed = await pdaOf(mark.programId, "feed", NPCS);
  const steps: { step: string; signature?: string; link?: string }[] = [];
  const record = (step: string, signature?: string) => {
    steps.push({ step, signature, link: signature ? explorer(signature, rpcUrl) : undefined });
    console.log(`● ${step}${signature ? `\n    ${explorer(signature, rpcUrl)}` : ""}`);
  };

  // 0. the engine may open rails sessions.
  const grant = await pdaOf(RAILS, "engine", operator);
  if (!(await rawAccount(ops.rpc, grant))) {
    record(
      `0 · rails grant_engine(auction operator ${operator}, ROLE_SESSIONS)`,
      await must(ops, "grant_engine", [
        railsIx("grant_engine", { config, grant, authority, system_program: SYSTEM }, { engine: operator, roles: ROLE_SESSIONS }),
      ]),
    );
  } else record("0 · auction operator already granted");

  // 1. buyers attested, validated, thawed; H's seasoned lots in FREE.
  const [X, Y] = [await generateKeyPairSigner(), await generateKeyPairSigner()];
  await must(ops, "fund buyers + H (SOL)", [X, Y, H].map((p) => transferSol(authority, p.address, SOL_EACH)));
  const authUsdc = await ensureAta(ops, authority.address, usdc, usdcTp);
  const acct: Record<"X" | "Y", { usdc: Address; npcs: Address }> = {} as never;
  for (const [name, who] of [["X", X], ["Y", Y]] as const) {
    const b = BIDS[name];
    const u = await ensureAta(ops, who.address, usdc, usdcTp);
    await must(ops, `fund ${name} USDC`, [transferToken(authUsdc, u, usdc, authority, gross(b.qty, b.limit) + BOND, usdcTp)]);
    const att = gateCli("sas.ts", ["attest", who.address, "--investor", crypto.randomUUID(), "--jurisdiction", "US", "--kyc", "2", "--accredited", "true", "--method", "1", "--days", "30"], rpcUrl, home(keys.authority));
    record(`1 · ${name} attested (SAS): ${att.split("\n").pop()}`);
    record(`1 · ${name} validated by credit-gate`, gateCli("wire.ts", ["validate", who.address, "--mint", NPCS], rpcUrl, home(keys.authority)).split("\n")[0]!);
    const n = await ensureAta(ops, who.address, NPCS, TOKEN_2022);
    record(`1 · ${name}'s NPCS account thawed through Token ACL (credit-gate decided)`, gateCli("wire.ts", ["thaw", n, "--mint", NPCS], rpcUrl, home(keys.authority)));
    acct[name] = { usdc: u, npcs: n };
  }
  const hUsdc = await ensureAta(ops, H.address, usdc, usdcTp);
  await must(ops, "fund H's ask bond", [transferToken(authUsdc, hUsdc, usdc, authority, BOND, usdcTp)]);
  const hNpcs = (await ownerTokenAccount(ops.rpc, H.address, NPCS))!;
  const hLots = await lotsPda(H.address);
  const before = sellable((await rawAccount(ops.rpc, hLots))!.data, await clockNow(ops.rpc));
  if (before.seasonedInLots > 0n) {
    record(`1 · H's seasoned lots merged into FREE (+${before.seasonedInLots})`, await must(ops, "merge_expired", [mergeExpiredIx(hLots, NPCS)]));
  }

  // 2. the session, sealed commits.
  const now = await clockNow(ops.rpc);
  const sessionId = now;
  const session = await pdaOf(AUCTION, "session", venue, le(sessionId, 8));
  const railsSession = await sessionPda(operator, sessionId);
  const commitEnd = now + BigInt(process.env.COMMIT_SECS ?? 240);
  const revealEnd = commitEnd + BigInt(process.env.REVEAL_SECS ?? 150);
  const params = {
    commit_end: commitEnd,
    reveal_end: revealEnd,
    rofr_window: 0n,
    settle_deadline: revealEnd + 3_600n,
    bond: BOND,
    max_cash: 2n * U,
    min_reveals: 2,
    max_discount_bps: MAX_DISCOUNT_BPS,
    forced_sale: false,
    rofr_authority: SYSTEM,
    disclosure_hash: new Uint8Array(32),
  };
  const opNoop = createNoopSigner(operator);
  const openRails = railsIx("open_session", { config, session: railsSession, asset_mint: NPCS, operator: opNoop, system_program: SYSTEM }, {
    session_id: sessionId,
    params: { commit_end: commitEnd, reveal_end: revealEnd, settle_deadline: params.settle_deadline, bond_amount: BOND, max_cash: params.max_cash, rofr_authority: SYSTEM },
  });
  record(
    `2 · auction session #${sessionId} opened over NPCS (rails cash session by CPI)`,
    await must(ops, "open_session", [
      withRails(
        auction.ix("open_session", { venue, session, feed, operator, opener: authority, rails_program: RAILS, system_program: SYSTEM }, { session_id: sessionId, params }),
        [...forwarded(openRails, operator), { address: grant, role: AccountRole.READONLY }],
      ),
    ]),
  );

  const nonces = { H: randomBytes(32), X: randomBytes(32), Y: randomBytes(32) };
  const hCtx = await makeCtx({ rpcUrl, payer: H, quiet: true });
  const hEscrow = await escrowPda(railsSession, H.address);
  const hAssetVault = await assetVaultPda(hEscrow);
  record("2 · H opened the escrow's NPCS vault (born frozen)", await must(hCtx, "open_asset_vault", [
    railsIx("open_asset_vault", { config, session: railsSession, escrow: hEscrow, asset_vault: hAssetVault, asset_mint: NPCS, owner: H, asset_token_program: TOKEN_2022, system_program: SYSTEM }),
  ]));
  record("2 · Token ACL authority thawed the escrow vault (a program PDA, not an investor)", await must(ops, "acl thaw", [await aclThawIx(acl, NPCS, hAssetVault)]));
  const askRails = railsIx("deposit_ask", {
    config, session: railsSession, escrow: hEscrow, vault: await vaultPda(hEscrow), asset_vault: hAssetVault, usdc_mint: usdc, asset_mint: NPCS,
    owner_usdc: hUsdc, owner_asset: hNpcs, owner: H, token_program: usdcTp, asset_token_program: TOKEN_2022, system_program: SYSTEM,
  }, { qty: ASK.qty });
  record("2 · COMMIT (sealed): H's ask, 1.0 NPCS escrowed", await must(hCtx, "commit_ask", [
    withRails(
      auction.ix("commit_ask", { session, owner: H, rails_program: RAILS }, { commitment_hash: commitment("Sell", ASK.limit, ASK.qty, nonces.H, H.address), qty: ASK.qty }),
      forwarded(askRails, operator),
    ),
  ]));
  for (const [name, who] of [["X", X], ["Y", Y]] as const) {
    const b = BIDS[name];
    const cash = gross(b.qty, b.limit);
    const escrow = await escrowPda(railsSession, who.address);
    const c = await makeCtx({ rpcUrl, payer: who, quiet: true });
    const dep = railsIx("deposit", { config, session: railsSession, escrow, vault: await vaultPda(escrow), usdc_mint: usdc, owner_token: acct[name].usdc, owner: who, token_program: usdcTp, system_program: SYSTEM }, { cash });
    record(`2 · COMMIT (sealed): ${name}'s bid, ${cash} µUSDC escrowed`, await must(c, `${name} commit_bid`, [
      withRails(
        auction.ix("commit_bid", { session, owner: who, rails_program: RAILS }, { commitment_hash: commitment("Buy", b.limit, b.qty, nonces[name], who.address), cash }),
        forwarded(dep, operator),
      ),
    ]));
  }

  // 3. reveals, then the phase crank: clear → allocate → settle (DvP through the gate) → finish (print).
  await waitUntil(ops, commitEnd, "commit window to close");
  const reveal = async (name: "H" | "X" | "Y", who: KeyPairSigner, limit: bigint, qty: bigint) => {
    const c = await makeCtx({ rpcUrl, payer: who, quiet: true });
    const markRevealed = railsIx("mark_revealed", { session: railsSession, escrow: await escrowPda(railsSession, who.address), operator: opNoop });
    record(`3 · REVEAL: ${name} (limit ${limit}, qty ${qty})`, await must(c, `${name} reveal`, [
      withRails(auction.ix("reveal", { venue, session, owner: who, operator, rails_program: RAILS }, { limit, qty, nonce: nonces[name] }), forwarded(markRevealed, operator)),
    ]));
  };
  await reveal("H", H, ASK.limit, ASK.qty);
  await reveal("X", X, BIDS.X.limit, BIDS.X.qty);
  await reveal("Y", Y, BIDS.Y.limit, BIDS.Y.qty);
  await waitUntil(ops, revealEnd, "reveal window to close");
  for (const o of await runPhases(ops, session, authority)) {
    if (o.result === "failed" || o.result === "skipped") throw new Error(`phase ${o.target}: ${o.detail}`);
    if (o.result === "sent") record(`3 · CRANK ${o.target}`, o.signature);
  }
  const s = await loadAuction(ops, session);
  if (s.phase !== "Printed") throw new Error(`session ended in ${s.phase}`);
  record(`3 · Printed: clear ${s.clear_price} µUSDC/NPCS, ${s.fills.length} fills settled DvP`);

  // 4. refunds, reconciliation, evidence.
  for (const r of await refundAll(ops, railsSession)) {
    if (r.result === "failed") throw new Error(`refund ${r.target}: ${r.detail}`);
    if (r.result === "sent") record(`4 · refund ${r.target.slice(0, 12)}…`, r.signature);
  }
  const rs = await reconcileSession(ops.rpc, railsSession);
  console.log("\n" + formatReport(rs));
  if (!rs.ok) throw new Error("reconciliation not clean");
  const evidenceDir = join(stateDir(rpcUrl), `evidence-auction-${sessionId}`);
  const manifest = await exportSessionEvidence(ops.rpc, evidenceDir, authority, session);
  const balances = {
    xNpcs: String(await tokenBalance(ops.rpc, acct.X.npcs)),
    yNpcs: String(await tokenBalance(ops.rpc, acct.Y.npcs)),
    hNpcs: String(await tokenBalance(ops.rpc, hNpcs)),
  };
  const out = writeJson(rpcUrl, `auction-${sessionId}.json`, {
    cluster: d.cluster, asset: NPCS, session, railsSession, clearPrice: String(s.clear_price), parties: { H: H.address, X: X.address, Y: Y.address },
    balances, reconciliation: rs.ok, evidence: { dir: evidenceDir, ...manifest }, steps,
  });
  console.log(`\n✓ live auction recorded → ${out}`);
}

if (import.meta.url === `file://${process.argv[1]}`) {
  main().catch((e) => {
    console.error("✗", e instanceof Error ? e.message : e);
    process.exit(1);
  });
}
