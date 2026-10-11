/**
 * The two participant writes on the session page: commit a sealed bid
 * (cash escrowed in payment-rails) and reveal it. Built client-side and
 * signed by the participant's own wallet; the server never signs (SRS 08 §6
 * api: "the server never custodies or signs user actions").
 *
 * Account order mirrors the programs (and the LiteSVM harness,
 * `payment-rails/tests-svm/tests/common/engines.rs`): the engine's own
 * accounts, then the rails accounts it forwards by CPI.
 */
import { AccountRole, type Address, type Instruction } from "@solana/kit";
import { concat, u32le, u64le } from "./borsh";
import { codecs, pdas, PROGRAMS, SYSTEM_PROGRAM } from "./programs";
import { getAddressEncoder } from "@solana/kit";

const enc = getAddressEncoder();
const disc = (name: string) =>
  Uint8Array.from(codecs.auction.idl.instructions.find((i) => i.name === name)!.discriminator);
const ro = (address: Address) => ({ address, role: AccountRole.READONLY });
const w = (address: Address) => ({ address, role: AccountRole.WRITABLE });

/** AE-P3: sha256(side ‖ limit ‖ qty ‖ nonce ‖ owner), as `auction_engine::commitment`. */
export async function commitment(
  side: "Buy" | "Sell",
  limit: bigint,
  qty: bigint,
  nonce: Uint8Array,
  owner: Address,
): Promise<Uint8Array> {
  if (nonce.length !== 32) throw new Error("nonce must be 32 bytes");
  const pre = concat(
    Uint8Array.of(side === "Buy" ? 0 : 1),
    u64le(limit),
    u64le(qty),
    nonce,
    enc.encode(owner) as Uint8Array,
  );
  return new Uint8Array(await crypto.subtle.digest("SHA-256", pre as BufferSource));
}

export interface BidAccounts {
  session: Address;
  railsSession: Address;
  owner: Address;
  usdcMint: Address;
  ownerUsdc: Address;
  usdcTokenProgram: Address;
}

/** `commit_bid(commitment, cash)`: escrows `cash` (+ the bond) in the rails. */
export async function commitBidIx(a: BidAccounts, commitmentHash: Uint8Array, cash: bigint): Promise<Instruction> {
  const escrow = await pdas.escrow(a.railsSession, a.owner);
  return {
    programAddress: PROGRAMS.auction,
    accounts: [
      w(a.session),
      { address: a.owner, role: AccountRole.WRITABLE_SIGNER },
      ro(PROGRAMS.rails),
      // payment-rails `deposit`, forwarded.
      ro(await pdas.railsConfig()),
      w(a.railsSession),
      w(escrow),
      w(await pdas.escrowVault(escrow)),
      ro(a.usdcMint),
      w(a.ownerUsdc),
      { address: a.owner, role: AccountRole.WRITABLE_SIGNER },
      ro(a.usdcTokenProgram),
      ro(SYSTEM_PROGRAM),
    ],
    data: concat(disc("commit_bid"), commitmentHash, u64le(cash)),
  };
}

/** `reveal(limit, qty, nonce)`: opens the preimage; the rails mark the escrow revealed. */
export async function revealIx(
  session: Address,
  railsSession: Address,
  owner: Address,
  limit: bigint,
  qty: bigint,
  nonce: Uint8Array,
): Promise<Instruction> {
  const operator = await pdas.operator();
  return {
    programAddress: PROGRAMS.auction,
    accounts: [
      ro(await pdas.venue()),
      w(session),
      { address: owner, role: AccountRole.READONLY_SIGNER },
      ro(operator),
      ro(PROGRAMS.rails),
      // payment-rails `mark_revealed`, forwarded (the engine signs for its operator PDA).
      ro(railsSession),
      w(await pdas.escrow(railsSession, owner)),
      ro(operator),
    ],
    data: concat(disc("reveal"), u64le(limit), u64le(qty), nonce),
  };
}

/** The token accounts and session terms one `exercise_rofr` moves value between. */
export interface RofrAccounts {
  session: Address;
  railsSession: Address;
  issuer: Address;
  seller: Address;
  assetMint: Address;
  usdcMint: Address;
  feeReceiver: Address;
  reserve: Address;
  /** Any USDC account the seller owns (rails: owner = escrow owner, mint = session USDC). */
  sellerUsdc: Address;
  issuerUsdc: Address;
  issuerAsset: Address;
  usdcTokenProgram: Address;
  assetTokenProgram: Address;
}

/**
 * AE-P7 `exercise_rofr(fill_index, qty)`: the issuer takes `qty` of fill
 * `fillIndex` at the clearing price, settled at once through the rails
 * `settle_rofr_fill` for the fill the engine appends (`newIndex` = current
 * fill count). The 23 forwarded accounts follow `SettleRofrFill` (and the
 * LiteSVM harness `exercise_rofr`); the operator PDA is de-signed, the
 * engine signs for it.
 */
export async function exerciseRofrIx(a: RofrAccounts, fillIndex: number, newIndex: number, qty: bigint): Promise<Instruction> {
  const operator = await pdas.operator();
  const sellerEscrow = await pdas.escrow(a.railsSession, a.seller);
  return {
    programAddress: PROGRAMS.auction,
    accounts: [
      ro(await pdas.venue()),
      w(a.session),
      { address: a.issuer, role: AccountRole.WRITABLE_SIGNER },
      w(operator),
      ro(PROGRAMS.rails),
      ro(SYSTEM_PROGRAM),
      // payment-rails `settle_rofr_fill`, forwarded.
      ro(await pdas.railsConfig()),
      w(a.railsSession),
      w(operator),
      { address: a.issuer, role: AccountRole.READONLY_SIGNER },
      w(await pdas.receipt(a.railsSession, newIndex)),
      w(sellerEscrow),
      w(await pdas.assetVault(sellerEscrow)),
      w(a.sellerUsdc),
      w(a.issuerUsdc),
      w(a.issuerAsset),
      w(a.feeReceiver),
      w(a.reserve),
      ro(a.usdcMint),
      ro(a.assetMint),
      ro(a.usdcTokenProgram),
      ro(a.assetTokenProgram),
      ro(SYSTEM_PROGRAM),
      // RofrGate
      ro(PROGRAMS.gate),
      ro(await pdas.gateSettle()),
      ro(await pdas.gateConfig(a.assetMint)),
      ro(await pdas.blocked(a.assetMint, a.seller)),
      ro(await pdas.blocked(a.assetMint, a.issuer)),
      w(await pdas.lots(a.assetMint, a.seller)),
    ],
    data: concat(disc("exercise_rofr"), u32le(fillIndex), u64le(qty)),
  };
}

/** AE-P7 `close_rofr`: the issuer declines (or is done); anyone once the window has passed. */
export function closeRofrIx(session: Address, caller: Address): Instruction {
  return {
    programAddress: PROGRAMS.auction,
    accounts: [w(session), { address: caller, role: AccountRole.READONLY_SIGNER }],
    data: disc("close_rofr"),
  };
}

/* ────────────────────────────────────────────────────────────────────────────
 * Crank: the settlement path.
 *
 * AE-P10 says every terminal path refunds, and the architecture puts phase
 * cranks on a bonded keeper (doc 06 §8). Both hold. These builders are the
 * permissionless on-chain cranks, so the session can also be driven from the
 * UI when no keeper is running — the same instructions, the same accounts, the
 * same order. `clear`, `expire` and `allocate` take no signer at all; `settle`
 * and `finish` need a payer for the account rent and fees.
 * ──────────────────────────────────────────────────────────────────────────── */

/** AE-P5 `clear`: the uniform clearing price, against the rails reserve floor. */
export async function clearIx(session: Address, feed: Address): Promise<Instruction> {
  return {
    programAddress: PROGRAMS.auction,
    accounts: [ro(await pdas.venue()), w(session), ro(feed), ro(await pdas.operator()), ro(PROGRAMS.rails)],
    data: disc("clear"),
  };
}

/** AE-P10 `expire`: past the settle deadline — refund everything, fail the session. */
export async function expireIx(session: Address, feed: Address): Promise<Instruction> {
  return {
    programAddress: PROGRAMS.auction,
    accounts: [ro(await pdas.venue()), w(session), ro(feed), ro(await pdas.operator()), ro(PROGRAMS.rails)],
    data: disc("expire"),
  };
}

/** AE-P6 `allocate`: pair the crossed book into fills, `maxFills` at a time (resumable). */
export function allocateIx(session: Address, maxFills = 32): Instruction {
  return {
    programAddress: PROGRAMS.auction,
    accounts: [w(session)],
    data: concat(disc("allocate"), Uint8Array.of(maxFills & 0xff)),
  };
}

/**
 * AE-P6 `screen_bid(order_index)`: the per-order gate check that runs BEFORE
 * the clear. An ineligible order is excluded with a typed `Exclusion` and its
 * volume is reallocated — it is never an error, so this is safe to crank over
 * the whole book. Without it every order stays `screened: false` and `clear`
 * would exclude the entire book.
 */
export interface ScreenBidAccounts {
  session: Address;
  assetMint: Address;
  buyer: Address;
  orderIndex: number;
  /** 32-byte `investor_id` from the buyer's identity attestation. */
  investorId: Uint8Array;
  credential: Address;
  identitySchema: Address;
  accreditationSchema: Address;
}

export async function screenBidIx(a: ScreenBidAccounts): Promise<Instruction> {
  return {
    programAddress: PROGRAMS.auction,
    accounts: [
      w(a.session),
      ro(await pdas.gateConfig(a.assetMint)),
      ro(await pdas.gatePolicy(a.assetMint)),
      ro(await pdas.blocked(a.assetMint, a.buyer)),
      ro(await pdas.holderLink(a.assetMint, a.buyer)),
      ro(await pdas.investor(a.assetMint, a.investorId)),
      ro(await pdas.lots(a.assetMint, a.buyer)),
      ro(await pdas.attestation(a.credential, a.identitySchema, a.buyer)),
      ro(await pdas.attestation(a.credential, a.accreditationSchema, a.buyer)),
      ro(a.assetMint),
      ro(PROGRAMS.gate),
    ],
    data: concat(disc("screen_bid"), Uint8Array.of(a.orderIndex & 0xff)),
  };
}

/** AE-P6 `screen_ask(order_index)`: the seller side — seasoning, block-list, holder link. */
export interface ScreenAskAccounts {
  session: Address;
  assetMint: Address;
  seller: Address;
  orderIndex: number;
}

export async function screenAskIx(a: ScreenAskAccounts): Promise<Instruction> {
  return {
    programAddress: PROGRAMS.auction,
    accounts: [
      w(a.session),
      ro(await pdas.gateConfig(a.assetMint)),
      ro(await pdas.blocked(a.assetMint, a.seller)),
      ro(await pdas.lots(a.assetMint, a.seller)),
      ro(await pdas.holderLink(a.assetMint, a.seller)),
    ],
    data: concat(disc("screen_ask"), Uint8Array.of(a.orderIndex & 0xff)),
  };
}

/** AE-P9 `finish`: close the book, write the `ClearingPrint` into mark-engine. */
export interface FinishAccounts {
  session: Address;
  feed: Address;
  assetMint: Address;
  /** `mark-engine` `Feed` authority — the print's author. */
  printAuthority: Address;
  payer: Address;
}

export async function finishIx(a: FinishAccounts): Promise<Instruction> {
  return {
    programAddress: PROGRAMS.auction,
    accounts: [
      ro(await pdas.venue()),
      w(a.session),
      w(a.feed),
      w(await pdas.printRecord(a.feed, a.session)),
      ro(a.printAuthority),
      ro(await pdas.operator()),
      { address: a.payer, role: AccountRole.WRITABLE_SIGNER },
      ro(PROGRAMS.rails),
      ro(PROGRAMS.mark),
      ro(SYSTEM_PROGRAM),
    ],
    data: disc("finish"),
  };
}

export interface CommitAskAccounts {
  session: Address;
  railsSession: Address;
  owner: Address;
  usdcMint: Address;
  assetMint: Address;
  /** The seller's USDC account (for the bond). */
  ownerUsdc: Address;
  /** The seller's NPCS account holding the offered units. */
  ownerAsset: Address;
  usdcTokenProgram: Address;
  assetTokenProgram: Address;
}

/**
 * AE-P3 `commit_ask(commitment_hash, qty)`: escrows the asset units AND the
 * bond through the rails `deposit_ask`, in one instruction. Cash and tokens are
 * pre-funded at commit, so there is no failed-fill risk at settlement (doc 06
 * §2, F2). A separate `open_asset_vault` may be needed first: NPCS is
 * default-frozen, so the rails' asset vault has to be thawed through the gate.
 */
export async function commitAskIx(a: CommitAskAccounts, commitmentHash: Uint8Array, qty: bigint): Promise<Instruction> {
  const escrow = await pdas.escrow(a.railsSession, a.owner);
  return {
    programAddress: PROGRAMS.auction,
    accounts: [
      w(a.session),
      { address: a.owner, role: AccountRole.WRITABLE_SIGNER },
      ro(PROGRAMS.rails),
      // payment-rails `deposit_ask`, forwarded.
      ro(await pdas.railsConfig()),
      w(a.railsSession),
      w(escrow),
      w(await pdas.escrowVault(escrow)),
      w(await pdas.assetVault(escrow)),
      ro(a.usdcMint),
      ro(a.assetMint),
      w(a.ownerUsdc),
      w(a.ownerAsset),
      { address: a.owner, role: AccountRole.WRITABLE_SIGNER },
      ro(a.usdcTokenProgram),
      ro(a.assetTokenProgram),
      ro(SYSTEM_PROGRAM),
    ],
    data: concat(disc("commit_ask"), commitmentHash, u64le(qty)),
  };
}

export interface SettleAccounts {
  session: Address;
  railsSession: Address;
  assetMint: Address;
  usdcMint: Address;
  /** Buyer of `fillIndex` — escrowed the cash. */
  buyer: Address;
  /** Seller of `fillIndex` — escrowed the units. */
  seller: Address;
  buyerAsset: Address;
  sellerUsdc: Address;
  /** From `RailsConfig`: the fee sink and the reserve. */
  feeReceiver: Address;
  reserve: Address;
  buyerTokenProgram: Address;
  assetTokenProgram: Address;
  /** Pays the receipt's rent and the transaction fee. */
  payer: Address;
  /** 32-byte `investor_id` of the buyer, for the gate's holder governor. */
  buyerInvestorId: Uint8Array;
  credential: Address;
  identitySchema: Address;
  accreditationSchema: Address;
}

/**
 * AE-P8 `settle(fill_index)`: ONE fill, atomic DvP — the cash leg and the asset
 * leg move in the same instruction, and the rails CPI credit-gate
 * `settle_check` BEFORE any value moves (doc 07 L4). One fill per instruction
 * keeps compute bounded and makes the whole loop crankable and resumable.
 *
 * The engine computes `qty`, `price` and `disclosure_hash` itself and forwards
 * them to the rails, so the caller supplies only the fill index. The 18
 * `settle_fill` accounts and the 11 `settle_check` accounts are passed as
 * remaining accounts; the operator PDA is de-signed because the engine signs
 * for it over CPI.
 */
export async function settleIx(a: SettleAccounts, fillIndex: number): Promise<Instruction> {
  const operator = await pdas.operator();
  const buyerEscrow = await pdas.escrow(a.railsSession, a.buyer);
  const sellerEscrow = await pdas.escrow(a.railsSession, a.seller);
  return {
    programAddress: PROGRAMS.auction,
    accounts: [
      ro(await pdas.venue()),
      w(a.session),
      w(operator),
      { address: a.payer, role: AccountRole.WRITABLE_SIGNER },
      ro(PROGRAMS.rails),
      ro(SYSTEM_PROGRAM),
      // payment-rails `settle_fill`, forwarded.
      ro(await pdas.railsConfig()),
      w(a.railsSession),
      w(operator),
      w(await pdas.receipt(a.railsSession, fillIndex)),
      w(buyerEscrow),
      w(await pdas.escrowVault(buyerEscrow)),
      w(sellerEscrow),
      w(await pdas.assetVault(sellerEscrow)),
      w(a.sellerUsdc),
      w(a.buyerAsset),
      w(a.feeReceiver),
      w(a.reserve),
      ro(a.usdcMint),
      ro(a.assetMint),
      ro(a.buyerTokenProgram),
      ro(a.assetTokenProgram),
      ro(SYSTEM_PROGRAM),
      ro(PROGRAMS.gate),
      // credit-gate `settle_check`, forwarded through the rails.
      ro(await pdas.gateSettle()),
      ro(await pdas.gateConfig(a.assetMint)),
      ro(await pdas.gatePolicy(a.assetMint)),
      ro(await pdas.blocked(a.assetMint, a.seller)),
      ro(await pdas.blocked(a.assetMint, a.buyer)),
      w(await pdas.lots(a.assetMint, a.seller)),
      w(await pdas.lots(a.assetMint, a.buyer)),
      ro(await pdas.holderLink(a.assetMint, a.buyer)),
      ro(await pdas.attestation(a.credential, a.identitySchema, a.buyer)),
      ro(await pdas.attestation(a.credential, a.accreditationSchema, a.buyer)),
      ro(a.assetMint),
    ],
    data: concat(disc("settle"), u32le(fillIndex)),
  };
}


const COMPUTE_BUDGET = "ComputeBudget111111111111111111111111111111" as Address;

/** `SetComputeUnitLimit`: an engine → rails → gate DvP needs more than the 200k default (the crank asks 600k). */
export function computeUnitLimitIx(units: number): Instruction {
  return { programAddress: COMPUTE_BUDGET, accounts: [], data: concat(Uint8Array.of(2), u32le(units)) };
}
