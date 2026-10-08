/**
 * R5 on live NPCS (#145, #172): one DvP fill where the asset leg is the
 * devnet NPCS mint `6vt1wHVQ…VKL4` (Token-2022, DefaultAccountState=Frozen,
 * gated by Token ACL → credit-gate), not a stand-in mint.
 *
 *   H (BJL7…2EMh, holds 1 NPCS, primary lot seasoning until 2026-10-11 07:5x UTC)
 *   sells 1.0 NPCS; X buys 0.6 and Y 0.4 at the clearing price. Every fill
 *   runs the rails `settle_fill`, which CPIs credit-gate `settle_check`
 *   (seller's lot seasoned, buyer admitted, block-list, attestations).
 *
 * Steps (every one is a real devnet transaction, recorded with its link):
 *   1. buyers X, Y: SAS attestations (registrar), `validate`, NPCS token
 *      accounts created frozen and thawed through Token ACL's permissionless
 *      thaw (credit-gate decides) — `ops/gate` sas.ts / wire.ts;
 *   2. rails session over the NPCS mint; H `open_asset_vault`, the Token ACL
 *      MintConfig authority thaws that escrow vault (it is a program PDA, not
 *      an investor: the gate never admits it — INTERFACE.md), H `deposit_ask`;
 *      X, Y `deposit`;
 *   3. reveals reported, two fills settled DvP through the gate;
 *   4. finish → refunds → reconcile → signed evidence bundle.
 *   (Before 2: H's seasoned lots are folded into FREE with `merge_expired`.)
 *
 *   npx tsx scripts/devnet/npcs-dvp.ts --preflight     # read-only: what still blocks the run
 *   npx tsx scripts/devnet/npcs-dvp.ts                 # the run
 *
 * Blocks, checked by --preflight:
 *   - devnet payment-rails must be the build that CPIs credit-gate 5ycY…X6XX:
 *     Squads proposal #2 (`ops/squads` `migrate.ts execute-upgrade`). The
 *     approved buffer FqAJ… is closed by the upgrade, so "buffer still
 *     exists" means "not executed". The DvP instructions are unchanged
 *     between that buffer's build and main (checked against the IDL).
 *   - H's lot must have seasoned (CG-P3, 7 days from 2026-10-04 07:5x UTC).
 *   - keys on this machine: rails operator (AUTHORITY), H, the Token ACL
 *     MintConfig authority 7Udt…, the SAS attester (ops/gate KEYS_DIR).
 *
 * Env: RPC_URL (devnet), AUTHORITY (rails operator, default
 * ~/.config/solana/turbine-devnet.json), HOLDER (default
 * ~/.config/nundina/npcs-devnet/holder-h.json), ACL_AUTHORITY (default
 * ~/.config/nundina/npcs-devnet/freeze-authority.json).
 */
import { execFileSync } from "node:child_process";
import { existsSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import {
  AccountRole,
  address,
  generateKeyPairSigner,
  getAddressEncoder,
  getProgramDerivedAddress,
  type Address,
  type Instruction,
  type AccountSignerMeta,
  type TransactionSigner,
} from "@solana/kit";
import { clockNow, ownerTokenAccount, rawAccount, tokenBalance } from "../../src/chain.ts";
import { finish, refundAll, settlePlan } from "../../src/crank.ts";
import { loadKeypair, makeCtx } from "../../src/ctx.ts";
import { exportEvidence } from "../../src/evidence.ts";
import { ix } from "../../src/idl.ts";
import { formatReport, reconcileSession } from "../../src/ledger.ts";
import { assetVaultPda, escrowPda, sessionPda, vaultPda } from "../../src/pda.ts";
import { CREDIT_GATE, mergeExpiredIx } from "../../src/seasoning.ts";
import { findAssociatedTokenPda } from "@solana-program/token";
import { ensureAta, explorer, must, readDeployment, stateDir, transferSol, transferToken, waitUntil, writeJson } from "./common.ts";

export const NPCS = address("6vt1wHVQVuhnfJ3FtNbq4647mCjWUBmJCDABsYUgVKL4");
export const TOKEN_2022 = address("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");
export const TOKEN_ACL = address("TACLkU6CiCdkQN2MjoyDkVg2yAH9zkxiHDsiztQ52TP");
/** The approved Squads upgrade's buffer (ops/squads/deployment-devnet.json); closed when executed. */
export const UPGRADE_BUFFER = address("FqAJqkS21CpfUZZZVuK2LMs679JPAukGQtMvAUeXBd5U");

const U = 1_000_000n;
const PRICE = (98n * U) / 100n; // 0.98 USDC per NPCS: a 2% discount to the 1.00 NAV
const FILLS = { X: (6n * U) / 10n, Y: (4n * U) / 10n };
const CASH = { X: (7n * U) / 10n, Y: U / 2n };
const BOND = U / 10n;
const SOL_EACH = 30_000_000n;
const OPS_GATE = new URL("../../../../ops/gate/", import.meta.url).pathname;
const home = (p: string) => p.replace(/^~(?=\/)/, homedir());

/**
 * Token ACL `Thaw` (instruction 4, authority-gated): accounts
 * [authority (signer), mint, token account (w), MintConfig, token program]
 * — solana-foundation/token-acl clients/js/src/generated/instructions/thaw.ts.
 */
export async function aclThawIx(authority: TransactionSigner, mint: Address, tokenAccount: Address): Promise<Instruction> {
  const [mintConfig] = await getProgramDerivedAddress({
    programAddress: TOKEN_ACL,
    seeds: [new TextEncoder().encode("MINT_CONFIG"), getAddressEncoder().encode(mint)],
  });
  return {
    programAddress: TOKEN_ACL,
    accounts: [
      { address: authority.address, role: AccountRole.READONLY_SIGNER, signer: authority } as AccountSignerMeta,
      { address: mint, role: AccountRole.READONLY },
      { address: tokenAccount, role: AccountRole.WRITABLE },
      { address: mintConfig, role: AccountRole.READONLY },
      { address: TOKEN_2022, role: AccountRole.READONLY },
    ],
    data: Uint8Array.of(4),
  };
}

/** HolderLots: lot_count @84, lots @85 × 31 bytes, restriction_expiry @+20; free_qty @72 (seasoning.ts layout). */
export function sellable(lots: Uint8Array, now: bigint): { free: bigint; seasonedInLots: bigint; nextExpiry: bigint | null } {
  const v = new DataView(lots.buffer, lots.byteOffset, lots.byteLength);
  const free = v.getBigUint64(72, true);
  let seasonedInLots = 0n;
  let nextExpiry: bigint | null = null;
  for (let i = 0; i < Math.min(lots[84]!, 16); i++) {
    const at = 85 + 31 * i;
    const qty = v.getBigUint64(at + 4, true);
    const exp = v.getBigInt64(at + 20, true);
    if (exp <= now) seasonedInLots += qty;
    else if (nextExpiry === null || exp < nextExpiry) nextExpiry = exp;
  }
  return { free, seasonedInLots, nextExpiry };
}

async function lotsPda(owner: Address): Promise<Address> {
  const enc = getAddressEncoder();
  const [a] = await getProgramDerivedAddress({
    programAddress: CREDIT_GATE,
    seeds: [new TextEncoder().encode("lots"), enc.encode(NPCS), enc.encode(owner)],
  });
  return a;
}

function gateCli(script: "sas.ts" | "wire.ts", args: string[], rpcUrl: string, payerPath: string): string {
  return execFileSync("npx", ["tsx", script, ...args], {
    cwd: OPS_GATE,
    env: { ...process.env, CLUSTER: "devnet", RPC_URL: rpcUrl, PAYER: payerPath },
    encoding: "utf8",
  }).trim();
}

async function preflight(rpcUrl: string, keyPaths: Record<string, string>): Promise<string[]> {
  const ctx = await makeCtx({ rpcUrl, payer: await generateKeyPairSigner(), quiet: true });
  const blocks: string[] = [];
  for (const [what, p] of Object.entries(keyPaths)) if (!existsSync(home(p))) blocks.push(`missing key: ${what} (${p})`);
  if (await rawAccount(ctx.rpc, UPGRADE_BUFFER)) {
    blocks.push(
      "devnet payment-rails is not upgraded: Squads proposal #2 is approved but not executed " +
        "(ops/squads: CLUSTER=devnet npx tsx migrate.ts execute-upgrade GdWmRivxe1k76apQB7EgJ4yBq8Fi3Lw25E8V3XosPocn)",
    );
  }
  const now = await clockNow(ctx.rpc);
  if (existsSync(home(keyPaths.holder!))) {
    const H = await loadKeypair(keyPaths.holder!);
    const lots = await rawAccount(ctx.rpc, await lotsPda(H.address));
    if (!lots) blocks.push(`H ${H.address} has no lot ledger`);
    else {
      const s = sellable(lots.data, now);
      if (s.free + s.seasonedInLots < U) {
        blocks.push(
          `H's NPCS is still seasoning (CG-P3): sellable ${s.free + s.seasonedInLots} < ${U}` +
            (s.nextExpiry !== null ? `; sellable from ${new Date(Number(s.nextExpiry) * 1000).toISOString()}` : ""),
        );
      }
      const hNpcs = await ownerTokenAccount(ctx.rpc, H.address, NPCS);
      if (!hNpcs || ((await tokenBalance(ctx.rpc, hNpcs)) ?? 0n) < U) blocks.push("H holds less than 1 NPCS");
    }
  }
  if (existsSync(home(keyPaths.authority!))) {
    const d = readDeployment(rpcUrl);
    const a = await loadKeypair(keyPaths.authority!);
    // The run spends from the operator's associated USDC account (ensureAta).
    const [usdc] = await findAssociatedTokenPda({ owner: a.address, mint: address(d.usdcMint), tokenProgram: address(d.usdcTokenProgram) });
    const need = CASH.X + CASH.Y + 3n * BOND + U / 10n;
    const have = (await tokenBalance(ctx.rpc, usdc)) ?? 0n;
    if (have < need) blocks.push(`rails operator ${a.address} holds ${have} µUSDC < ${need} needed (https://faucet.circle.com)`);
    const sol = (await ctx.rpc.getBalance(a.address).send()).value;
    if (sol < 300_000_000n) blocks.push(`rails operator has ${sol} lamports < 0.3 SOL`);
  }
  return blocks;
}

async function main(): Promise<void> {
  const rpcUrl = process.env.RPC_URL ?? "https://api.devnet.solana.com";
  const keyPaths = {
    authority: process.env.AUTHORITY ?? "~/.config/solana/turbine-devnet.json",
    holder: process.env.HOLDER ?? "~/.config/nundina/npcs-devnet/holder-h.json",
    aclAuthority: process.env.ACL_AUTHORITY ?? "~/.config/nundina/npcs-devnet/freeze-authority.json",
    sasAttester: process.env.SAS_ATTESTER ?? "~/.config/nundina/sas-devnet/attester.json",
  };
  const blocks = await preflight(rpcUrl, keyPaths);
  if (process.argv.includes("--preflight") || blocks.length > 0) {
    console.log(blocks.length ? `NOT READY:\n  - ${blocks.join("\n  - ")}` : "READY: every precondition holds");
    process.exit(blocks.length ? 1 : 0);
  }

  const d = readDeployment(rpcUrl);
  const authority = await loadKeypair(keyPaths.authority);
  const H = await loadKeypair(keyPaths.holder);
  const acl = await loadKeypair(keyPaths.aclAuthority);
  const ops = await makeCtx({ rpcUrl, payer: authority });
  const usdc = address(d.usdcMint);
  const usdcTp = address(d.usdcTokenProgram);
  const config = address(d.config);
  const steps: { step: string; signature?: string; link?: string }[] = [];
  const record = (step: string, signature?: string) => {
    steps.push({ step, signature, link: signature ? explorer(signature, rpcUrl) : undefined });
    console.log(`● ${step}${signature ? `\n    ${explorer(signature, rpcUrl)}` : ""}`);
  };

  // 1. buyers: attested, validated, NPCS accounts thawed by the gate.
  const [X, Y] = [await generateKeyPairSigner(), await generateKeyPairSigner()];
  await must(ops, "fund buyers + H (SOL)", [X, Y, H].map((p) => transferSol(authority, p.address, SOL_EACH)));
  const authUsdc = await ensureAta(ops, authority.address, usdc, usdcTp);
  const acct: Record<string, { usdc: Address; npcs: Address }> = {};
  for (const [name, who, cash] of [["X", X, CASH.X], ["Y", Y, CASH.Y]] as const) {
    const u = await ensureAta(ops, who.address, usdc, usdcTp);
    await must(ops, `fund ${name} USDC`, [transferToken(authUsdc, u, usdc, authority, cash + BOND, usdcTp)]);
    const investor = crypto.randomUUID();
    record(`1 · ${name} attested (SAS): ${gateCli("sas.ts", ["attest", who.address, "--investor", investor, "--jurisdiction", "US", "--kyc", "2", "--accredited", "true", "--method", "1", "--days", "30"], rpcUrl, home(keyPaths.authority)).split("\n").pop()}`);
    const v = gateCli("wire.ts", ["validate", who.address, "--mint", NPCS], rpcUrl, home(keyPaths.authority)).split("\n")[0]!;
    record(`1 · ${name} validated by credit-gate`, v);
    const n = await ensureAta(ops, who.address, NPCS, TOKEN_2022);
    const t = gateCli("wire.ts", ["thaw", n, "--mint", NPCS], rpcUrl, home(keyPaths.authority));
    record(`1 · ${name}'s NPCS account thawed through Token ACL (permissionless; credit-gate decided)`, t);
    acct[name] = { usdc: u, npcs: n };
  }
  const hUsdc = await ensureAta(ops, H.address, usdc, usdcTp);
  await must(ops, "fund H's ask bond", [transferToken(authUsdc, hUsdc, usdc, authority, BOND, usdcTp)]);
  const hNpcs = (await ownerTokenAccount(ops.rpc, H.address, NPCS))!;

  // 1b. fold H's seasoned lots into FREE (CG-P3 `merge_expired`, permissionless:
  // the seasoning crank's job). `debit` would also merge on demand inside
  // settle_check; doing it here makes the ask sell from FREE explicitly and
  // checks it before any escrow is opened.
  const hLots = await lotsPda(H.address);
  const before = sellable((await rawAccount(ops.rpc, hLots))!.data, await clockNow(ops.rpc));
  if (before.seasonedInLots > 0n) {
    record(`1 · H's seasoned lots merged into FREE (+${before.seasonedInLots})`, await must(ops, "merge_expired", [mergeExpiredIx(hLots, NPCS)]));
  }
  const after = sellable((await rawAccount(ops.rpc, hLots))!.data, await clockNow(ops.rpc));
  if (after.free < U) throw new Error(`H's FREE is ${after.free} after merge_expired; the ask needs ${U}`);

  // 2. session over NPCS; H's ask; buyers' cash.
  const now = await clockNow(ops.rpc);
  const sessionId = now;
  const session = await sessionPda(authority.address, sessionId);
  const [commitEnd, revealEnd] = [now + 90n, now + 120n];
  record(
    "2 · rails session opened, asset leg = NPCS 6vt1…VKL4",
    await must(ops, "open session", [
      ix("open_session", { config, session, asset_mint: NPCS, operator: authority }, {
        session_id: sessionId,
        params: { commit_end: commitEnd, reveal_end: revealEnd, settle_deadline: revealEnd + 1_800n, bond_amount: BOND, max_cash: 5n * U, rofr_authority: address("11111111111111111111111111111111") },
      }),
    ]),
  );
  const hCtx = await makeCtx({ rpcUrl, payer: H, quiet: true });
  const hEscrow = await escrowPda(session, H.address);
  const hAssetVault = await assetVaultPda(hEscrow);
  record("2 · H opened the escrow's NPCS vault (created frozen)", await must(hCtx, "open_asset_vault", [
    ix("open_asset_vault", { config, session, escrow: hEscrow, asset_vault: hAssetVault, asset_mint: NPCS, owner: H, asset_token_program: TOKEN_2022 }),
  ]));
  const aclCtx = await makeCtx({ rpcUrl, payer: authority, quiet: true });
  record("2 · Token ACL authority thawed the escrow vault (a program PDA, never an investor)", await must(aclCtx, "acl thaw", [
    await aclThawIx(acl, NPCS, hAssetVault),
  ]));
  record("2 · COMMIT: H escrowed 1.0 NPCS + bond", await must(hCtx, "deposit_ask", [
    ix("deposit_ask", {
      config, session, escrow: hEscrow, vault: await vaultPda(hEscrow), asset_vault: hAssetVault, usdc_mint: usdc, asset_mint: NPCS,
      owner_usdc: hUsdc, owner_asset: hNpcs, owner: H, token_program: usdcTp, asset_token_program: TOKEN_2022,
    }, { qty: U }),
  ]));
  for (const [name, who, cash] of [["X", X, CASH.X], ["Y", Y, CASH.Y]] as const) {
    const escrow = await escrowPda(session, who.address);
    const c = await makeCtx({ rpcUrl, payer: who, quiet: true });
    record(`2 · COMMIT: ${name} escrowed ${cash} µUSDC + bond`, await must(c, `${name} deposit`, [
      ix("deposit", { config, session, escrow, vault: await vaultPda(escrow), usdc_mint: usdc, owner_token: acct[name]!.usdc, owner: who, token_program: usdcTp }, { cash }),
    ]));
  }

  // 3. reveal → settle DvP through the gate.
  await waitUntil(ops, commitEnd, "commit window to close");
  for (const who of [H, X, Y]) {
    await must(ops, "mark_revealed", [ix("mark_revealed", { session, escrow: await escrowPda(session, who.address), operator: authority })]);
  }
  record("3 · reveals reported for H, X, Y");
  await waitUntil(ops, revealEnd, "reveal window to close");
  const fills = await settlePlan(ops, session, authority, [
    { index: 0, buyer: X.address, seller: H.address, qty: FILLS.X, price: PRICE, sellerUsdc: hUsdc, buyerAsset: acct.X!.npcs },
    { index: 1, buyer: Y.address, seller: H.address, qty: FILLS.Y, price: PRICE, sellerUsdc: hUsdc, buyerAsset: acct.Y!.npcs },
  ]);
  for (const f of fills) {
    if (f.result !== "sent" && f.result !== "already-done") throw new Error(`${f.target}: ${f.result} ${f.detail ?? ""}`);
    record(`3 · SETTLE (DvP, NPCS leg, gate settle_check): ${f.target}`, f.signature);
  }

  // 4. finish → refunds → proof.
  record("4 · settlement finished", (await finish(ops, session, authority)).signature);
  for (const r of await refundAll(ops, session)) {
    if (r.result === "failed") throw new Error(`refund ${r.target}: ${r.detail}`);
    record(`4 · refund ${r.target.slice(0, 12)}… (${r.result})`, r.signature);
  }
  const rs = await reconcileSession(ops.rpc, session);
  console.log("\n" + formatReport(rs));
  if (!rs.ok) throw new Error("reconciliation not clean");
  const balances = {
    xNpcs: (await tokenBalance(ops.rpc, acct.X!.npcs))?.toString(),
    yNpcs: (await tokenBalance(ops.rpc, acct.Y!.npcs))?.toString(),
    hNpcs: (await tokenBalance(ops.rpc, hNpcs))?.toString(),
  };
  const evidenceDir = join(stateDir(rpcUrl), `evidence-npcs-${sessionId}`);
  const manifest = await exportEvidence(ops.rpc, evidenceDir, authority, { limit: 200 });
  console.log(`evidence: ${manifest.events} events, ${JSON.stringify(manifest.transferProof)}, sha256 ${manifest.sha256} → ${evidenceDir}`);
  const out = writeJson(rpcUrl, `npcs-dvp-${sessionId}.json`, {
    cluster: "devnet", asset: NPCS, session, parties: { H: H.address, X: X.address, Y: Y.address }, price: PRICE.toString(),
    balances, reconciliation: rs.ok, evidence: { dir: evidenceDir, sha256: manifest.sha256, transferProof: manifest.transferProof }, steps,
  });
  console.log(`\n✓ NPCS DvP recorded → ${out}`);
}

if (import.meta.url === `file://${process.argv[1]}`) {
  main().catch((e) => {
    console.error("✗", e instanceof Error ? e.message : e);
    process.exit(1);
  });
}
