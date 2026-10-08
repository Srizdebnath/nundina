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
import { concat, u64le } from "./borsh";
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
