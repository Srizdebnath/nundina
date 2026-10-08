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

const COMPUTE_BUDGET = "ComputeBudget111111111111111111111111111111" as Address;

/** `SetComputeUnitLimit`: an engine → rails → gate DvP needs more than the 200k default (the crank asks 600k). */
export function computeUnitLimitIx(units: number): Instruction {
  return { programAddress: COMPUTE_BUDGET, accounts: [], data: concat(Uint8Array.of(2), u32le(units)) };
}
