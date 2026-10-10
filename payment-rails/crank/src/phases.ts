/**
 * Phase-deadline crank for auction-engine sessions (SRS 08 §6 crank-runner,
 * AE-P2/P10, #149).
 *
 * The engine's phases advance only when someone calls the permissionless
 * instruction for the next step: `screen_bid` / `screen_ask` on every
 * revealed order once reveals close (AE-P6 gate check), `clear`, `allocate`
 * in chunks, `close_rofr` once the ROFR window lapses, `settle` per fill,
 * `finish` (rails `finish_session` + the print into mark-engine), and
 * `expire` past the hard deadline. This crank reads the session, asks
 * {@link planPhase} what the next step is, and sends it — repeatedly, until
 * nothing is due. Like the settlement crank, it holds no state of its own:
 * a restarted or competing crank re-reads the chain and resumes.
 *
 * Any wallet can run it (it pays receipt / print rent).
 */
import {
  AccountRole,
  address,
  createNoopSigner,
  getAddressEncoder,
  getProgramDerivedAddress,
  type Address,
  type TransactionSigner,
} from "@solana/kit";
import { CrankLookupTable } from "./alt.ts";
import { clockNow, ownerTokenAccount, rawAccount, tokenProgramOf } from "./chain.ts";
import type { Outcome } from "./crank.ts";
import {
  blockEntryPda,
  fillGateAccounts,
  gateConfigPda,
  gatePolicyPda,
  holderLinkPda,
  holderLotsPda,
  screenGateAccounts,
} from "./gate.ts";
import { decodeAccount, ix as railsIx, loadIdl, makeCodec, PROGRAM_ID as RAILS, type Struct } from "./idl.ts";
import { assetVaultPda, configPda, escrowPda, receiptPda, vaultPda } from "./pda.ts";
import { send, type Ctx } from "./send.ts";

export const auction = makeCodec(loadIdl("auction_engine"));
export const AUCTION = auction.programId;
export const MARK_ENGINE = address("79ypVjMMCDbmxYWaXTTgxz9jidjncoJAhHQC6e13x4LH");

export interface Fill extends Struct {
  buyer: Address;
  seller: Address;
  qty: bigint;
  rofr: boolean;
  settled: boolean;
}
export interface Order extends Struct {
  owner: Address;
  side: string;
  revealed: boolean;
  screened: boolean;
}
export interface AuctionSession extends Struct {
  session_id: bigint;
  asset_mint: Address;
  feed: Address;
  rails_session: Address;
  params: Struct & { reveal_end: bigint; rofr_window: bigint; settle_deadline: bigint };
  phase: string;
  clear_price: bigint;
  orders: Order[];
  fills: Fill[];
  /** AE-P6 / AE-P9 (#202): the gate investor behind each order. */
  investor_ids: Uint8Array[];
  /** AE-P2 (#202): bit i = fill i retired by `skip_fill`. */
  skipped_fills: number;
}

export type PhaseAction =
  | { kind: "screen"; index: number; side: "Buy" | "Sell" }
  | { kind: "clear" }
  | { kind: "allocate" }
  | { kind: "close_rofr" }
  | { kind: "settle"; index: number }
  | { kind: "skip"; index: number }
  | { kind: "finish" }
  | { kind: "expire" };

/**
 * Pure: the next permissionless step for `s` at `now`, or `null` when the
 * session waits on a participant (commits, reveals, the issuer's ROFR
 * window) or is terminal. Mirrors the program's guards exactly, so the
 * crank never sends a step that will fail on timing.
 */
export function planPhase(s: AuctionSession, now: bigint): PhaseAction | null {
  const p = s.params;
  if (s.phase === "Printed" || s.phase === "Failed") return null;
  if (now >= p.settle_deadline) return { kind: "expire" };
  switch (s.phase) {
    case "Commit":
    case "Reveal":
    case "Clearing": {
      if (now < p.reveal_end) return null;
      // AE-P6: every revealed order is screened before the clear.
      const i = s.orders.findIndex((o) => o.revealed && !o.screened);
      if (i < 0) return { kind: "clear" };
      return { kind: "screen", index: i, side: s.orders[i]!.side === "Sell" ? "Sell" : "Buy" };
    }
    case "Allocating":
      return { kind: "allocate" };
    case "IssuerRofr":
      return now >= p.reveal_end + p.rofr_window ? { kind: "close_rofr" } : null;
    case "Settling": {
      const i = s.fills.findIndex((f) => !f.settled);
      return i >= 0 ? { kind: "settle", index: i } : { kind: "finish" };
    }
    default:
      return null;
  }
}

const enc = getAddressEncoder();
const seedBytes = (x: string | Address | Uint8Array) =>
  typeof x === "string" && x.length < 32 ? new TextEncoder().encode(x) : x instanceof Uint8Array ? x : enc.encode(x as Address);
async function pdaOf(program: Address, ...seeds: (string | Address | Uint8Array)[]): Promise<Address> {
  const [a] = await getProgramDerivedAddress({ programAddress: program, seeds: seeds.map(seedBytes) });
  return a;
}

export async function loadAuction(ctx: Ctx, session: Address): Promise<AuctionSession> {
  const raw = await rawAccount(ctx.rpc, session);
  const s = raw && raw.owner === AUCTION ? auction.decodeAccount<AuctionSession>("Session", raw.data) : null;
  if (!s) throw new Error(`${session} is not an auction-engine session`);
  return s;
}

type Meta = { address: Address; role: AccountRole };

/** The rails `OperatorSession` accounts (session, operator PDA unsigned). */
function railsSessionAccounts(s: AuctionSession, operator: Address): Meta[] {
  return [
    { address: s.rails_session, role: AccountRole.WRITABLE },
    { address: operator, role: AccountRole.READONLY },
  ];
}

/** Build the engine instruction for `a`, rails accounts forwarded. */
async function buildStep(ctx: Ctx, sessionAddr: Address, s: AuctionSession, a: PhaseAction, payer: TransactionSigner) {
  const venue = await pdaOf(AUCTION, "venue");
  const operator = await pdaOf(AUCTION, "rails_operator");
  const base = { venue, session: sessionAddr, operator, rails_program: RAILS };
  const withRails = (instruction: ReturnType<typeof auction.ix>, extra: Meta[]) => ({
    ...instruction,
    accounts: [...(instruction.accounts ?? []), ...extra],
  });
  switch (a.kind) {
    case "screen": {
      const owner = s.orders[a.index]!.owner;
      const gate = await screenGateAccounts(ctx.rpc, s.asset_mint, owner);
      if (a.side === "Sell") {
        return auction.ix(
          "screen_ask",
          {
            session: sessionAddr,
            gate_config: gate.gate_config,
            block_entry: gate.block_entry,
            seller_lots: gate.lots,
            holder_link: gate.holder_link,
          },
          { order_index: a.index },
        );
      }
      return auction.ix(
        "screen_bid",
        {
          session: sessionAddr,
          gate_config: gate.gate_config,
          gate_policy: gate.gate_policy,
          block_entry: gate.block_entry,
          holder_link: gate.holder_link,
          investor: gate.investor,
          buyer_lots: gate.lots,
          identity_attestation: gate.identity,
          accreditation_attestation: gate.accreditation,
          mint: s.asset_mint,
          gate_program: gate.gate_program,
        },
        { order_index: a.index },
      );
    }
    case "allocate":
      return auction.ix("allocate", { session: sessionAddr }, { max_fills: 8 });
    case "close_rofr":
      return auction.ix("close_rofr", { session: sessionAddr, caller: payer });
    case "skip": {
      const f = s.fills[a.index]!;
      const gate = await fillGateAccounts(ctx.rpc, s.asset_mint, f.seller, f.buyer);
      // No holder link: the buyer's link address stands in for it and its
      // attestations (the program sees no link → NotAdmitted).
      const g = "reason" in gate ? null : gate.accounts;
      const link = g?.buyer_link ?? (await holderLinkPda(s.asset_mint, f.buyer));
      return auction.ix(
        "skip_fill",
        {
          session: sessionAddr,
          gate_config: await gateConfigPda(s.asset_mint),
          gate_policy: await gatePolicyPda(s.asset_mint),
          seller_block: await blockEntryPda(s.asset_mint, f.seller),
          buyer_block: await blockEntryPda(s.asset_mint, f.buyer),
          buyer_link: link,
          identity_attestation: g?.buyer_identity ?? link,
          accreditation_attestation: g?.buyer_accreditation ?? link,
          seller_lots: await holderLotsPda(s.asset_mint, f.seller),
          buyer_lots: await holderLotsPda(s.asset_mint, f.buyer),
        },
        { fill_index: a.index },
      );
    }
    case "clear":
    case "expire":
      return withRails(auction.ix(a.kind, { ...base, feed: s.feed }), railsSessionAccounts(s, operator));
    case "finish": {
      const print = await pdaOf(MARK_ENGINE, "print", s.feed, sessionAddr);
      return withRails(
        auction.ix("finish", {
          ...base,
          feed: s.feed,
          print_record: print,
          print_authority: await pdaOf(AUCTION, "print"),
          payer,
        }),
        railsSessionAccounts(s, operator),
      );
    }
    case "settle": {
      const f = s.fills[a.index]!;
      const rs = s.rails_session;
      const rails = await rawAccount(ctx.rpc, rs);
      if (!rails) throw new Error("rails session missing");
      const usdcMint = decodeAccount<Struct & { usdc_mint: Address; fee_receiver: Address; reserve: Address }>(
        "CashSession",
        rails.data,
      );
      if (!usdcMint) throw new Error("rails session undecodable");
      const assetMint = s.asset_mint;
      const [usdcProgram, assetProgram] = await Promise.all([
        tokenProgramOf(ctx.rpc, usdcMint.usdc_mint),
        tokenProgramOf(ctx.rpc, assetMint),
      ]);
      const [sellerUsdc, buyerAsset] = await Promise.all([
        ownerTokenAccount(ctx.rpc, f.seller, usdcMint.usdc_mint),
        ownerTokenAccount(ctx.rpc, f.buyer, assetMint),
      ]);
      if (!sellerUsdc || !buyerAsset) throw new Error("seller USDC or buyer asset account missing");
      const gate = await fillGateAccounts(ctx.rpc, assetMint, f.seller, f.buyer);
      if ("reason" in gate) throw new Error(gate.reason);
      const [buyerEscrow, sellerEscrow] = await Promise.all([escrowPda(rs, f.buyer), escrowPda(rs, f.seller)]);
      // The rails instruction, built by its own IDL with the operator PDA
      // as a no-op signer, then forwarded with the PDA de-signed (the
      // engine signs for it).
      const inner = railsIx(
        "settle_fill",
        {
          config: await configPda(),
          session: rs,
          operator: createNoopSigner(operator),
          receipt: await receiptPda(rs, a.index),
          buyer_escrow: buyerEscrow,
          buyer_vault: await vaultPda(buyerEscrow),
          seller_escrow: sellerEscrow,
          seller_asset_vault: await assetVaultPda(sellerEscrow),
          seller_usdc: sellerUsdc,
          buyer_asset: buyerAsset,
          fee_receiver: usdcMint.fee_receiver,
          reserve: usdcMint.reserve,
          usdc_mint: usdcMint.usdc_mint,
          asset_mint: assetMint,
          token_program: usdcProgram,
          asset_token_program: assetProgram,
          gate: gate.accounts,
        },
        { fill_index: a.index, qty: 0n, price: 0n, disclosure_hash: new Uint8Array(32) },
      );
      const forwarded: Meta[] = (inner.accounts ?? []).map((m) => ({
        address: m.address,
        role: m.address === operator ? AccountRole.WRITABLE : m.role,
      }));
      return withRails(auction.ix("settle", { ...base, payer }, { fill_index: a.index }), forwarded);
    }
  }
}

/**
 * Drive `session` forward until nothing is due (or `maxSteps`). Every step
 * is re-planned from fresh chain state.
 */
export async function runPhases(
  ctx: Ctx,
  session: Address,
  payer: TransactionSigner,
  opts: { maxSteps?: number } = {},
): Promise<Outcome[]> {
  const out: Outcome[] = [];
  // `settle` is too wide for a plain transaction (see alt.ts): one table per run.
  const alt = new CrankLookupTable();
  for (let step = 0; step < (opts.maxSteps ?? 64); step++) {
    const s = await loadAuction(ctx, session);
    const a = planPhase(s, await clockNow(ctx.rpc));
    if (!a) {
      out.push({ target: "phase", result: "already-done", detail: `waiting in ${s.phase}` });
      break;
    }
    const target = a.kind === "settle" || a.kind === "screen" ? `${a.kind} #${a.index}` : a.kind;
    let instruction;
    try {
      instruction = await buildStep(ctx, session, s, a, payer);
    } catch (e) {
      out.push({ target, result: "skipped", detail: (e as Error).message });
      break;
    }
    let lookupTables;
    if (a.kind === "settle") {
      try {
        lookupTables = await alt.cover(ctx, [instruction]);
      } catch (e) {
        out.push({ target, result: "failed", detail: (e as Error).message });
        break;
      }
    }
    const r = await send(ctx, `auction ${target}`, [instruction], { cuLimit: 600_000, lookupTables });
    if (r.status !== "sent") {
      // AE-P2: a fill that fails the gate's settle_check is retired with
      // `skip_fill` (which re-runs the same checks and refuses a fill that
      // would pass), so the session can still finish. Anything else stops.
      if (a.kind === "settle") {
        const skip: PhaseAction = { kind: "skip", index: a.index };
        const k = await send(ctx, `auction skip #${a.index}`, [await buildStep(ctx, session, s, skip, payer)], {
          cuLimit: 400_000,
        });
        if (k.status === "sent") {
          out.push({ target: `skip #${a.index}`, result: "sent", signature: k.signature, detail: r.name ?? r.message });
          continue;
        }
      }
      out.push({ target, result: "failed", detail: r.name ?? r.message });
      break;
    }
    out.push({ target, result: "sent", signature: r.signature });
  }
  return out;
}

