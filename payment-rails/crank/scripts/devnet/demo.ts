/**
 * R5 demo — commit → settle → payout, end to end on a live cluster.
 *
 * The queue-claim story, cash side (PILOT-DEMO-ASSET stands in for the
 * claim until NPCS/queue-claim land — the asset is per-session, so swapping
 * it in later needs no program change):
 *
 *  0. Holder H is admitted to the payout registry (hash-only attestation).
 *  1. PAYOUT (facility): H pledges the claim; the facility advances USDC —
 *     paid only to H's registered wallet.
 *  2. COMMIT: H asks to sell the claim; buyers X, Y bid (cash + bonds
 *     escrowed, bids sealed — the rails only see cash amounts).
 *  3. REVEAL → SETTLE: the crank settles two DvP fills at the clearing
 *     price (claim → buyers, cash → H's proceeds account, fee, reserve).
 *  4. FINISH → REFUNDS: buyers' excess + all bonds come back (crank, but
 *     any wallet could do it).
 *  5. PAYOUT (sweep, QC-P4): H's sale proceeds repay the facility first;
 *     the remainder goes to H's registered wallet.
 *  6. Reconcile session + facility; export evidence (SHA-256 manifest).
 *
 * Needs: deploy.sh + setup.ts done on the cluster; the authority holding
 * ≥ 9 USDC of the configured settlement mint and ~0.3 SOL. Since #133
 * every fill CPIs credit-gate: the demo asset needs a gate market, the
 * buyers a `validate`, and H a seasoned lot (ops/gate wire.ts, scripts/
 * drill-gate.ts shows the sequence), and the cluster's credit-gate must
 * carry `settle_check` (devnet: the gate is live at 5ycY…X6XX, #170; the
 * deployed payment-rails still names the superseded gate id until its
 * Squads upgrade). Until then settlePlan skips each fill ("buyer not admitted").
 *   RPC_URL=https://api.devnet.solana.com npx tsx scripts/devnet/demo.ts
 */
import { join } from "node:path";
import { address, generateKeyPairSigner, type Address, type KeyPairSigner } from "@solana/kit";
import { getCreateAccountInstruction } from "@solana-program/system";
import {
  getInitializeAccount3Instruction,
  getInitializeMintInstruction,
  getMintSize,
  getMintToInstruction,
  getTokenSize,
  TOKEN_PROGRAM_ADDRESS,
} from "@solana-program/token";
import { clockNow, fetchDecoded, tokenBalance } from "../../src/chain.ts";
import { finish, refundAll, settlePlan } from "../../src/crank.ts";
import { loadKeypair, makeCtx } from "../../src/ctx.ts";
import { exportEvidence } from "../../src/evidence.ts";
import { ix, type Struct } from "../../src/idl.ts";
import { formatReport, reconcileFacility, reconcileSession } from "../../src/ledger.ts";
import {
  advancePda, assetVaultPda, destinationPda, escrowPda, facilityPda, facilityVaultPda, sessionPda, vaultPda,
} from "../../src/pda.ts";
import {
  ensureAta, explorer, must, readDeployment, stateDir, transferSol, transferToken, waitUntil, writeJson,
} from "./common.ts";

const U = 1_000_000n; // 6 dp
const FACILITY_FUND = 5n * U;
const ADVANCE = 2n * U;
const BOND = U / 10n;
const PRICE = 3n * U; // clearing price, USDC per whole claim unit
const BIDS = { X: 2n * U, Y: 3n * U / 2n };
const FILLS = { X: (6n * U) / 10n, Y: (4n * U) / 10n }; // 0.6 + 0.4 of H's 1.0
const NEEDED_USDC = FACILITY_FUND + BIDS.X + BIDS.Y + 2n * BOND + U / 2n; // + margin
const SOL_EACH = 30_000_000n; // 0.03 SOL per participant

interface Step { step: string; signature?: string; link?: string; detail?: string }
const steps: Step[] = [];
function record(rpcUrl: string, step: string, signature?: string, detail?: string): void {
  steps.push({ step, signature, link: signature ? explorer(signature, rpcUrl) : undefined, detail });
  console.log(`● ${step}${detail ? ` — ${detail}` : ""}${signature ? `\n    ${explorer(signature, rpcUrl)}` : ""}`);
}
const fmt = (v: bigint | null) => (v === null ? "—" : `${v / U}.${(v % U).toString().padStart(6, "0")}`);

async function main(): Promise<void> {
  const rpcUrl = process.env.RPC_URL ?? "https://api.devnet.solana.com";
  const d = readDeployment(rpcUrl);
  const authority = await loadKeypair(process.env.AUTHORITY ?? "~/.config/solana/id.json");
  if (authority.address !== d.authority) throw new Error(`AUTHORITY is ${authority.address}, deployment authority is ${d.authority}`);
  const ops = await makeCtx({ rpcUrl, payer: authority });
  const usdc = address(d.usdcMint);
  const usdcTp = address(d.usdcTokenProgram);
  const config = address(d.config);
  const commitSecs = BigInt(process.env.COMMIT_SECS ?? 45);
  const revealSecs = BigInt(process.env.REVEAL_SECS ?? 30);

  // ── preflight ────────────────────────────────────────────────────────────
  const authUsdc = await ensureAta(ops, authority.address, usdc, usdcTp);
  const have = (await tokenBalance(ops.rpc, authUsdc)) ?? 0n;
  if (have < NEEDED_USDC) {
    throw new Error(
      `authority USDC ${fmt(have)} < needed ${fmt(NEEDED_USDC)}. Fund ${authUsdc} (owner ${authority.address}) with the settlement mint ${usdc}` +
        (d.cluster === "devnet" ? " — https://faucet.circle.com (Solana devnet)" : ""),
    );
  }
  const sol = (await ops.rpc.getBalance(authority.address).send()).value;
  if (sol < 300_000_000n) throw new Error(`authority has ${sol} lamports; needs ≥ 0.3 SOL`);
  console.log(`demo on ${d.cluster} · authority ${authority.address} · USDC ${fmt(have)}\n`);

  // ── participants & the claim asset ───────────────────────────────────────
  const [H, X, Y] = [await generateKeyPairSigner(), await generateKeyPairSigner(), await generateKeyPairSigner()];
  await must(ops, "fund participants (SOL)", [H, X, Y].map((p) => transferSol(authority, p.address, SOL_EACH)));

  const rent = async (n: number) => ops.rpc.getMinimumBalanceForRentExemption(BigInt(n)).send();
  const claimMint = await generateKeyPairSigner();
  await must(ops, "create PILOT-DEMO-ASSET mint", [
    getCreateAccountInstruction({ payer: authority, newAccount: claimMint, lamports: await rent(getMintSize()), space: getMintSize(), programAddress: TOKEN_PROGRAM_ADDRESS }),
    getInitializeMintInstruction({ mint: claimMint.address, decimals: 6, mintAuthority: authority.address }),
  ]);
  const asset = claimMint.address;
  const ata = async (owner: Address, mint: Address, tp: Address) => ensureAta(ops, owner, mint, tp);
  const hPayout = await ata(H.address, usdc, usdcTp); // registered payout wallet
  const hClaim = await ata(H.address, asset, TOKEN_PROGRAM_ADDRESS);
  const [xUsdc, xClaim, yUsdc, yClaim] = [
    await ata(X.address, usdc, usdcTp), await ata(X.address, asset, TOKEN_PROGRAM_ADDRESS),
    await ata(Y.address, usdc, usdcTp), await ata(Y.address, asset, TOKEN_PROGRAM_ADDRESS),
  ];
  // H's separate proceeds account (the queue-claim engine's proceeds bucket).
  const hProceedsKey = await generateKeyPairSigner();
  await must(ops, "create H proceeds account", [
    getCreateAccountInstruction({ payer: authority, newAccount: hProceedsKey, lamports: await rent(getTokenSize()), space: getTokenSize(), programAddress: usdcTp }),
    getInitializeAccount3Instruction({ account: hProceedsKey.address, mint: usdc, owner: H.address }, { programAddress: usdcTp }),
  ]);
  const hProceeds = hProceedsKey.address;
  await must(ops, "mint 1.0 claim to H; fund buyers", [
    getMintToInstruction({ mint: asset, token: hClaim, mintAuthority: authority, amount: U }),
    transferToken(authUsdc, xUsdc, usdc, authority, BIDS.X + BOND, usdcTp),
    transferToken(authUsdc, yUsdc, usdc, authority, BIDS.Y + BOND, usdcTp),
  ]);

  // ── 0. payout registry ──────────────────────────────────────────────────
  const hDest = await destinationPda(H.address);
  const now0 = await clockNow(ops.rpc);
  const regSig = await must(ops, "register H payout destination", [
    ix("register_destination", { config, destination: hDest, owner: H.address, token_account: hPayout, registrar: authority }, {
      attestation_ref_hash: new Uint8Array(32).fill(0xa7), expires_at: now0 + 30n * 86_400n,
    }),
  ]);
  record(rpcUrl, "0 · H admitted to payout registry (attestation hash only)", regSig);

  // ── 1. payout: facility advance against the pledged claim ────────────────
  const facilityId = now0;
  const facility = await facilityPda(authority.address, facilityId);
  const fVault = await facilityVaultPda(facility);
  await must(ops, "open facility", [
    ix("open_facility", { config, facility, vault: fVault, usdc_mint: usdc, operator: authority, token_program: usdcTp }, { facility_id: facilityId, rate_bps: 1_200n }),
  ]);
  await must(ops, "fund facility", [
    ix("fund_facility", { facility, vault: fVault, usdc_mint: usdc, funder_token: authUsdc, funder: authority, token_program: usdcTp }, { amount: FACILITY_FUND }),
  ]);
  const advSig = await must(ops, "advance to H", [
    ix("advance_funds", {
      config, facility, advance: await advancePda(facility, H.address), vault: fVault, borrower: H.address,
      destination: hDest, destination_token: hPayout, usdc_mint: usdc, curator: authority, token_program: usdcTp,
    }, { amount: ADVANCE, collateral_ref_hash: new Uint8Array(32).fill(0xc1), collateral_value: 2n * ADVANCE, advance_rate_bps: 5_000n }),
  ]);
  record(rpcUrl, `1 · PAYOUT: facility advanced ${fmt(ADVANCE)} USDC to H's registered wallet`, advSig);

  // ── 2. commit ───────────────────────────────────────────────────────────
  const sessionId = now0;
  const session = await sessionPda(authority.address, sessionId);
  const t = await clockNow(ops.rpc);
  const [commitEnd, revealEnd, deadline] = [t + commitSecs, t + commitSecs + revealSecs, t + commitSecs + revealSecs + 900n];
  const openSig = await must(ops, "open session", [
    ix("open_session", { config, session, asset_mint: asset, operator: authority }, {
      session_id: sessionId,
      params: { commit_end: commitEnd, reveal_end: revealEnd, settle_deadline: deadline, bond_amount: BOND, max_cash: 20n * U, rofr_authority: address("11111111111111111111111111111111") },
    }),
  ]);
  record(rpcUrl, "2 · session opened (deadlines snapshotted)", openSig);

  const hEscrow = await escrowPda(session, H.address);
  const hCtx = await makeCtx({ rpcUrl, payer: H, quiet: true });
  const hAssetVault = await assetVaultPda(hEscrow);
  // The stand-in asset isn't frozen-by-default, so open + deposit go in one
  // transaction. With NPCS the gate thaws the vault between the two.
  const askSig = await must(hCtx, "H deposit_ask", [
    ix("open_asset_vault", {
      config, session, escrow: hEscrow, asset_vault: hAssetVault, asset_mint: asset, owner: H,
      asset_token_program: TOKEN_PROGRAM_ADDRESS,
    }),
    ix("deposit_ask", {
      config, session, escrow: hEscrow, vault: await vaultPda(hEscrow), asset_vault: hAssetVault,
      usdc_mint: usdc, asset_mint: asset, owner_usdc: hPayout, owner_asset: hClaim, owner: H,
      token_program: usdcTp, asset_token_program: TOKEN_PROGRAM_ADDRESS,
    }, { qty: U }),
  ]);
  record(rpcUrl, "2 · COMMIT: H escrowed 1.0 claim + bond", askSig);
  for (const [who, key, acct, cash] of [["X", X, xUsdc, BIDS.X], ["Y", Y, yUsdc, BIDS.Y]] as const) {
    const escrow = await escrowPda(session, key.address);
    const c = await makeCtx({ rpcUrl, payer: key, quiet: true });
    const sig = await must(c, `${who} deposit`, [
      ix("deposit", { config, session, escrow, vault: await vaultPda(escrow), usdc_mint: usdc, owner_token: acct, owner: key, token_program: usdcTp }, { cash }),
    ]);
    record(rpcUrl, `2 · COMMIT: ${who} escrowed ${fmt(cash)} USDC + bond (bid sealed)`, sig);
  }

  // ── 3. reveal → settle ──────────────────────────────────────────────────
  await waitUntil(ops, commitEnd, "commit window to close");
  for (const key of [H, X, Y]) {
    await must(ops, "mark_revealed", [ix("mark_revealed", { session, escrow: await escrowPda(session, key.address), operator: authority })]);
  }
  record(rpcUrl, "3 · reveals reported for H, X, Y");
  await waitUntil(ops, revealEnd, "reveal window to close");
  const fills = await settlePlan(ops, session, authority, [
    { index: 0, buyer: X.address, seller: H.address, qty: FILLS.X, price: PRICE, sellerUsdc: hProceeds, buyerAsset: xClaim },
    { index: 1, buyer: Y.address, seller: H.address, qty: FILLS.Y, price: PRICE, sellerUsdc: hProceeds, buyerAsset: yClaim },
  ]);
  for (const f of fills) {
    // "already-done": the send landed but its confirmation was lost and the
    // retry hit the fill receipt — idempotency working, not a failure.
    if (f.result !== "sent" && f.result !== "already-done") throw new Error(`${f.target}: ${f.result} ${f.detail ?? ""}`);
    record(rpcUrl, `3 · SETTLE (DvP): ${f.target}${f.result === "already-done" ? " (landed; confirmation lost, receipt verified)" : ""}`, f.signature);
  }

  // Exact sale proceeds, captured before refunds (H's bond refund may land
  // in any of H's USDC accounts — the program only requires H owns it).
  const proceeds = (await tokenBalance(ops.rpc, hProceeds)) ?? 0n;

  // ── 4. finish → refunds ─────────────────────────────────────────────────
  const fin = await finish(ops, session, authority);
  record(rpcUrl, "4 · settlement finished", fin.signature);
  for (const r of await refundAll(ops, session)) {
    if (r.result === "failed") throw new Error(`refund ${r.target}: ${r.detail}`);
    record(rpcUrl, `4 · refund ${r.target.slice(0, 12)}… (${r.result})`, r.signature, r.detail);
  }

  // ── 5. payout: sweep proceeds — facility first, then H ──────────────────
  const before = { vault: (await tokenBalance(ops.rpc, fVault))!, h: (await tokenBalance(ops.rpc, hPayout))! };
  const sweepSig = await must(hCtx, "sweep proceeds", [
    ix("sweep_proceeds", {
      facility, advance: await advancePda(facility, H.address), vault: fVault, curator: authority.address,
      proceeds_token: hProceeds, proceeds_authority: H, holder_destination: hDest, holder_token: hPayout,
      usdc_mint: usdc, token_program: usdcTp,
    }, { amount: proceeds }),
  ]);
  const toFacility = (await tokenBalance(ops.rpc, fVault))! - before.vault;
  const toHolder = (await tokenBalance(ops.rpc, hPayout))! - before.h;
  record(rpcUrl, `5 · PAYOUT (sweep): proceeds ${fmt(proceeds)} → facility ${fmt(toFacility)} first, H ${fmt(toHolder)}`, sweepSig);
  if (toFacility + toHolder !== proceeds) throw new Error("sweep legs don't sum to proceeds");

  // ── 6. proof ────────────────────────────────────────────────────────────
  const rs = await reconcileSession(ops.rpc, session);
  const rf = await reconcileFacility(ops.rpc, facility);
  console.log("\n" + formatReport(rs) + "\n" + formatReport(rf));
  if (!rs.ok || !rf.ok) throw new Error("reconciliation not clean");
  const evidenceDir = join(stateDir(rpcUrl), `evidence-${sessionId}`);
  const manifest = await exportEvidence(ops.rpc, evidenceDir, authority, { limit: 300 });
  console.log(`evidence: ${manifest.events} events, sha256 ${manifest.sha256}, signed by ${manifest.attestation.signer} → ${evidenceDir}`);

  // Hygiene: return the participants' leftover SOL to the authority.
  for (const key of [H, X, Y]) {
    const c = await makeCtx({ rpcUrl, payer: key, quiet: true });
    const bal = (await c.rpc.getBalance(key.address).send()).value;
    const keep = 1_000_000n; // fee headroom
    if (bal > keep) await must(c, "return SOL", [transferSol(key, authority.address, bal - keep)], 5_000);
  }

  const f = await fetchDecoded<Struct>(ops.rpc, "Facility", facility);
  const summary = {
    cluster: d.cluster, programId: d.programId, session, facility, claimMint: asset,
    parties: { H: H.address, X: X.address, Y: Y.address },
    balances: {
      hPayoutUsdc: fmt(await tokenBalance(ops.rpc, hPayout)),
      xClaim: fmt(await tokenBalance(ops.rpc, xClaim)), yClaim: fmt(await tokenBalance(ops.rpc, yClaim)),
      facilityInterest: fmt((f?.interest_collected as bigint) ?? 0n),
    },
    reconciliation: { session: rs.ok, facility: rf.ok },
    evidence: { dir: evidenceDir, events: manifest.events, sha256: manifest.sha256 },
    steps,
  };
  console.log(`\n✓ demo complete → ${writeJson(rpcUrl, `demo-${sessionId}.json`, summary)}`);
}

main().catch((e) => {
  console.error("✗", e instanceof Error ? e.message : e);
  process.exit(1);
});
