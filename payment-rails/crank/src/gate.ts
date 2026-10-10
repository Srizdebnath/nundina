/**
 * credit-gate accounts a fill passes to `settle_fill` (AE-P8, #133/#134).
 *
 * The rails CPI credit-gate's `settle_check` inside every fill: block-list on
 * both wallets, the buyer's SAS attestations re-read, the seller's seasoned
 * lots debited and the buyer's lot appended. The program re-derives every
 * one of these from the asset mint and the two wallets, so a wrong account
 * fails the fill; nothing here is trusted.
 *
 * The buyer's attestation addresses come from its `HolderLink` (written by
 * the gate's `validate`): discriminator 8 · mint 32 · owner 32 ·
 * identity_attestation @72 · accreditation_attestation @104, the offsets
 * `HolderLink::IDENTITY_OFFSET` pins on the Rust side.
 */
import { getAddressDecoder, getAddressEncoder, getProgramDerivedAddress, type Address } from "@solana/kit";
import { rawAccount, type R } from "./chain.ts";
import { PROGRAM_ID } from "./idl.ts";
import { CREDIT_GATE } from "./seasoning.ts";

const utf8 = new TextEncoder();
const enc = getAddressEncoder();

async function gatePda(literal: string, ...keys: (Address | Uint8Array)[]): Promise<Address> {
  const [a] = await getProgramDerivedAddress({
    programAddress: CREDIT_GATE,
    seeds: [utf8.encode(literal), ...keys.map((k) => (k instanceof Uint8Array ? k : enc.encode(k)))],
  });
  return a;
}

export const gateConfigPda = (mint: Address) => gatePda("config", mint);
export const gatePolicyPda = (mint: Address) => gatePda("policy", mint);
export const blockEntryPda = (mint: Address, wallet: Address) => gatePda("blocked", mint, wallet);
export const holderLotsPda = (mint: Address, wallet: Address) => gatePda("lots", mint, wallet);
export const holderLinkPda = (mint: Address, wallet: Address) => gatePda("holder", mint, wallet);
/** The rails' settlement PDA: the only signer credit-gate takes `settle_check` from. */
export async function gateSettlePda(): Promise<Address> {
  const [a] = await getProgramDerivedAddress({ programAddress: PROGRAM_ID, seeds: [utf8.encode("gate_settle")] });
  return a;
}

export const investorRecordPda = (mint: Address, investorId: Uint8Array) => gatePda("investor", mint, investorId);

export const HOLDER_LINK_IDENTITY_AT = 72;
export const HOLDER_LINK_ACCREDITATION_AT = 104;
/** `HolderLink::investor_id` ([u8; 16]) follows the two attestations. */
export const HOLDER_LINK_INVESTOR_ID_AT = 136;

/** Pure: the two attestation addresses a `HolderLink` rests on. */
export function linkAttestations(data: Uint8Array): { identity: Address; accreditation: Address } | null {
  if (data.length < HOLDER_LINK_ACCREDITATION_AT + 32) return null;
  const dec = getAddressDecoder();
  return {
    identity: dec.decode(data.subarray(HOLDER_LINK_IDENTITY_AT, HOLDER_LINK_IDENTITY_AT + 32)),
    accreditation: dec.decode(data.subarray(HOLDER_LINK_ACCREDITATION_AT, HOLDER_LINK_ACCREDITATION_AT + 32)),
  };
}

/**
 * The `settle_fill` gate accounts for one fill, or a reason it can't settle
 * (the buyer was never admitted by `validate`).
 */
export async function fillGateAccounts(
  rpc: R,
  mint: Address,
  seller: Address,
  buyer: Address,
): Promise<{ accounts: Record<string, Address> } | { reason: string }> {
  const link = await holderLinkPda(mint, buyer);
  const raw = await rawAccount(rpc, link);
  const atts = raw && raw.owner === CREDIT_GATE ? linkAttestations(raw.data) : null;
  if (!atts) return { reason: "buyer not admitted by credit-gate (no holder link)" };
  return {
    accounts: {
      gate_program: CREDIT_GATE,
      gate_authority: await gateSettlePda(),
      gate_config: await gateConfigPda(mint),
      gate_policy: await gatePolicyPda(mint),
      seller_block: await blockEntryPda(mint, seller),
      buyer_block: await blockEntryPda(mint, buyer),
      seller_lots: await holderLotsPda(mint, seller),
      buyer_lots: await holderLotsPda(mint, buyer),
      buyer_link: link,
      buyer_identity: atts.identity,
      buyer_accreditation: atts.accreditation,
    },
  };
}

/**
 * The credit-gate accounts auction-engine's AE-P6 `screen_bid` /
 * `screen_ask` read for `wallet`. A wallet with no `HolderLink` still gets a
 * full set (the link address stands in for the attestations and investor
 * record): the program sees no link and excludes the bid as NotAdmitted
 * before it looks at the rest. Every address is re-derived on-chain.
 */
export async function screenGateAccounts(rpc: R, mint: Address, wallet: Address) {
  const link = await holderLinkPda(mint, wallet);
  const raw = await rawAccount(rpc, link);
  const admitted = raw && raw.owner === CREDIT_GATE && raw.data.length >= HOLDER_LINK_INVESTOR_ID_AT + 16;
  const atts = admitted ? linkAttestations(raw.data) : null;
  const investorId = admitted ? raw.data.subarray(HOLDER_LINK_INVESTOR_ID_AT, HOLDER_LINK_INVESTOR_ID_AT + 16) : null;
  return {
    gate_program: CREDIT_GATE,
    gate_config: await gateConfigPda(mint),
    gate_policy: await gatePolicyPda(mint),
    block_entry: await blockEntryPda(mint, wallet),
    lots: await holderLotsPda(mint, wallet),
    holder_link: link,
    investor: investorId ? await investorRecordPda(mint, investorId) : link,
    identity: atts?.identity ?? link,
    accreditation: atts?.accreditation ?? link,
  };
}
