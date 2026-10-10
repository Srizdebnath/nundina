/**
 * Program ids, codecs and PDAs. Ids come from the IDLs (`declare_id!`), so
 * they are the ids the programs were built with; the cluster and the NPCS
 * mint come from the environment (see `.env.example`).
 */
import { getAddressEncoder, getProgramDerivedAddress, address, type Address } from "@solana/kit";
import auctionIdl from "@/idl/auction_engine.json";
import gateIdl from "@/idl/credit_gate.json";
import markIdl from "@/idl/mark_engine.json";
import queueIdl from "@/idl/queue_claim.json";
import facilityIdl from "@/idl/liquidity_facility.json";
import trancheIdl from "@/idl/tranche_engine.json";
import treasuryIdl from "@/idl/npcs_treasury.json";
import railsIdl from "@/idl/payment_rails.json";
import { Codec, u32le, u64le, type Idl } from "./borsh";

export const codecs = {
  auction: new Codec(auctionIdl as Idl),
  gate: new Codec(gateIdl as Idl),
  mark: new Codec(markIdl as Idl),
  queue: new Codec(queueIdl as Idl),
  facility: new Codec(facilityIdl as Idl),
  tranche: new Codec(trancheIdl as Idl),
  treasury: new Codec(treasuryIdl as Idl),
  rails: new Codec(railsIdl as Idl),
};

export const PROGRAMS = {
  auction: address(auctionIdl.address),
  gate: address(gateIdl.address),
  mark: address(markIdl.address),
  queue: address(queueIdl.address),
  facility: address(facilityIdl.address),
  tranche: address(trancheIdl.address),
  treasury: address(treasuryIdl.address),
  rails: address(railsIdl.address),
} as const;

export const SAS_PROGRAM = address("22zoJMtdu4tQc2PzL74ZUT7FrwgB1Udec8DdW4yw4BdG");
export const SYSTEM_PROGRAM = address("11111111111111111111111111111111");
export const TOKEN_PROGRAM = address("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
export const TOKEN_2022_PROGRAM = address("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");

export const CLUSTER = process.env.NEXT_PUBLIC_CLUSTER ?? "devnet";
export const RPC_URL =
  process.env.NEXT_PUBLIC_RPC_URL ??
  (CLUSTER === "localnet" ? "http://127.0.0.1:8899" : "https://api.devnet.solana.com");
/** NPCS devnet mint (`token/deployment-devnet.json`, #171). */
export const NPCS_MINT = address(
  process.env.NEXT_PUBLIC_NPCS_MINT ?? "6vt1wHVQVuhnfJ3FtNbq4647mCjWUBmJCDABsYUgVKL4",
);

export function explorer(kind: "address" | "tx", id: string): string {
  const q =
    CLUSTER === "localnet"
      ? `?cluster=custom&customUrl=${encodeURIComponent(RPC_URL)}`
      : CLUSTER === "mainnet-beta"
        ? ""
        : `?cluster=${CLUSTER}`;
  return `https://explorer.solana.com/${kind}/${id}${q}`;
}

const enc = getAddressEncoder();
/** An address as a seed (string seeds are UTF-8 literals). */
const A = (a: Address) => enc.encode(a) as Uint8Array;

async function pda(program: Address, seeds: (string | Uint8Array)[]): Promise<Address> {
  const bytes = seeds.map((s) => (typeof s === "string" ? new TextEncoder().encode(s) : s));
  const [a] = await getProgramDerivedAddress({ programAddress: program, seeds: bytes });
  return a;
}

export const pdas = {
  // credit-gate
  gateConfig: (mint: Address) => pda(PROGRAMS.gate, ["config", A(mint)]),
  gatePolicy: (mint: Address) => pda(PROGRAMS.gate, ["policy", A(mint)]),
  blocked: (mint: Address, w: Address) => pda(PROGRAMS.gate, ["blocked", A(mint), A(w)]),
  holderLink: (mint: Address, w: Address) => pda(PROGRAMS.gate, ["holder", A(mint), A(w)]),
  investor: (mint: Address, id: Uint8Array) => pda(PROGRAMS.gate, ["investor", A(mint), id]),
  lots: (mint: Address, w: Address) => pda(PROGRAMS.gate, ["lots", A(mint), A(w)]),
  blocklist: (mint: Address) => pda(PROGRAMS.gate, ["blocklist", A(mint)]),
  gateRoot: () => pda(PROGRAMS.gate, ["gate_root"]),
  attestation: (credential: Address, schema: Address, owner: Address) =>
    pda(SAS_PROGRAM, ["attestation", A(credential), A(schema), A(owner)]),
  // auction-engine
  venue: () => pda(PROGRAMS.auction, ["venue"]),
  auctionSession: async (id: bigint) => pda(PROGRAMS.auction, ["session", A(await pda(PROGRAMS.auction, ["venue"])), u64le(id)]),
  operator: () => pda(PROGRAMS.auction, ["rails_operator"]),
  // mark-engine
  feed: (mint: Address) => pda(PROGRAMS.mark, ["feed", A(mint)]),
  printRecord: (feed: Address, session: Address) => pda(PROGRAMS.mark, ["print", A(feed), A(session)]),
  // payment-rails
  railsConfig: () => pda(PROGRAMS.rails, ["rails_config"]),
  escrow: (railsSession: Address, owner: Address) => pda(PROGRAMS.rails, ["escrow", A(railsSession), A(owner)]),
  escrowVault: (escrow: Address) => pda(PROGRAMS.rails, ["vault", A(escrow)]),
  assetVault: (escrow: Address) => pda(PROGRAMS.rails, ["asset_vault", A(escrow)]),
  receipt: (railsSession: Address, fillIndex: number) => pda(PROGRAMS.rails, ["fill", A(railsSession), u32le(fillIndex)]),
  /** The rails' settlement signer: the only caller credit-gate takes `settle_check` from. */
  gateSettle: () => pda(PROGRAMS.rails, ["gate_settle"]),
  engineGrant: (engine: Address) => pda(PROGRAMS.rails, ["engine", A(engine)]),
  /** A payout destination (`["payout", owner]`): where facility advances may go. */
  payout: (owner: Address) => pda(PROGRAMS.rails, ["payout", A(owner)]),
  /** A rails facility fund (`["facility", curator, id]`). */
  railsFacility: (curator: Address, id: bigint) => pda(PROGRAMS.rails, ["facility", A(curator), u64le(id)]),
  facilityVault: (facility: Address) => pda(PROGRAMS.rails, ["facility_vault", A(facility)]),
  advance: (facility: Address, borrower: Address) => pda(PROGRAMS.rails, ["advance", A(facility), A(borrower)]),
  // credit-gate custody (LF-P5)
  custodian: (mint: Address, custodian: Address) => pda(PROGRAMS.gate, ["custodian", A(mint), A(custodian)]),
  /** The holder key a pledge's lot ledger is kept under (`credit_gate::pledge_holder`). */
  pledgeHolder: (mint: Address, custodian: Address, pledgeRef: Address) =>
    pda(PROGRAMS.gate, ["pledge", A(mint), A(custodian), A(pledgeRef)]),
  // queue-claim
  queue: (mint: Address) => pda(PROGRAMS.queue, ["queue", A(mint)]),
  queueRequest: (queue: Address, id: bigint) => pda(PROGRAMS.queue, ["request", A(queue), u64le(id)]),
  queueConfig: (queue: Address) => pda(PROGRAMS.queue, ["queue_cfg", A(queue)]),
  queueNpcsVault: (request: Address) => pda(PROGRAMS.queue, ["npcs", A(request)]),
  queueSigner: () => pda(PROGRAMS.queue, ["qc_signer"]),
  // liquidity-facility
  lfSigner: () => pda(PROGRAMS.facility, ["lf_signer"]),
  lfConfig: (id: bigint) => pda(PROGRAMS.facility, ["lf_facility", u64le(id)]),
  position: (config: Address, borrower: Address) => pda(PROGRAMS.facility, ["position", A(config), A(borrower)]),
  collateral: (position: Address) => pda(PROGRAMS.facility, ["collateral", A(position)]),
  proceeds: (position: Address) => pda(PROGRAMS.facility, ["proceeds", A(position)]),
  // tranche-engine
  teSigner: () => pda(PROGRAMS.tranche, ["te_signer"]),
  // npcs-treasury
  treasury: (mint: Address) => pda(PROGRAMS.treasury, ["treasury", A(mint)]),
  treasuryVault: (treasury: Address) => pda(PROGRAMS.treasury, ["vault", A(treasury)]),
  treasurySigner: (treasury: Address) => pda(PROGRAMS.treasury, ["treasury_signer", A(treasury)]),
};

export const ASSOCIATED_TOKEN_PROGRAM = address("ATokenGPvbdGVxr1b2hvZbsjzLKkGSBqZp6GbsJ6VCh7");

/** The associated token account of `owner` for `mint` under `tokenProgram`. */
export const ata = (owner: Address, mint: Address, tokenProgram: Address) =>
  pda(ASSOCIATED_TOKEN_PROGRAM, [A(owner), A(tokenProgram), A(mint)]);
