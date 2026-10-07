/**
 * indexer (SRS 08 §6): decode every program event into normalized tables.
 *
 * Two feeds, one ingestion path:
 *   - RPC backfill (`backfill`): per program, `getSignaturesForAddress`
 *     from the stored cursor, oldest first, `getTransaction`, ingest. It is
 *     also the gap-fill: run it after any webhook outage.
 *   - webhooks (`webhookHandler`): Helius "raw" transaction webhooks (or any
 *     sender posting the same shape), pushed as they land.
 *
 * Failed transactions are skipped (their logs can carry events of
 * instructions that were rolled back). Re-ingesting a transaction is a
 * no-op: events are keyed by (signature, event_index) and every projection
 * is a SET, phases only move forward, and state columns take the latest
 * slot.
 *
 * An event whose bytes match a discriminator but not the IDL layout (a
 * deployment that predates the IDL) is stored raw (`data.undecoded`) and not
 * projected: nothing is dropped, and it can be re-decoded with the old IDL.
 *
 * Limitation: RPC nodes truncate logs past ~10 KB ("Log truncated"); events
 * after the cut are not in the logs and so not indexed. Every instruction
 * here emits far less than that.
 */
import type { IncomingMessage, ServerResponse } from "node:http";
import type { Address, Signature } from "@solana/kit";
import type { Struct } from "./codec.ts";
import type { Db } from "./db.ts";
import { eventsFromLogs, plain, type DecodedEvent } from "./logs.ts";
import { BY_ID } from "./programs.ts";
import type { R } from "./rpc.ts";

export interface RawTx {
  signature: string;
  slot: bigint | number;
  blockTime: bigint | number | null;
  err: unknown;
  logs: readonly string[];
}

const s = (v: unknown) => (v === null || v === undefined ? null : typeof v === "bigint" ? v.toString() : v);
const hex = (b: unknown) => plain(b) as string;
const variant = (v: unknown) => (typeof v === "string" ? v : v && typeof v === "object" ? Object.keys(v)[0]! : String(v));
const PHASES = ["Commit", "Reveal", "Allocating", "IssuerRofr", "Settling", "Printed", "Failed"];
/** Printed and Failed are both terminal and exclusive on-chain: once either is stored, a late event never replaces it. */
const TERMINAL = ["Printed", "Failed"];
const LOT_KIND = ["append", "split", "merge"];

/** Store a transaction's events and project them. Returns how many events it had. */
export async function ingest(db: Db, tx: RawTx): Promise<number> {
  if (tx.err) return 0;
  const events = eventsFromLogs(tx.logs);
  const slot = BigInt(tx.slot);
  for (const ev of events) {
    await db.query(
      `INSERT INTO events (signature, event_index, slot, block_time, program, name, data)
       VALUES ($1, $2, $3, $4, $5, $6, $7) ON CONFLICT DO NOTHING`,
      [
        tx.signature,
        ev.index,
        slot.toString(),
        tx.blockTime === null ? null : String(tx.blockTime),
        ev.program,
        ev.name,
        JSON.stringify(ev.undecoded ? { undecoded: true, raw: ev.undecoded } : plain(ev.data)),
      ],
    );
  }
  for (const ev of events) if (!ev.undecoded) await project(db, tx.signature, slot, ev, events);
  return events.length;
}

async function project(db: Db, sig: string, slot: bigint, ev: DecodedEvent, all: DecodedEvent[]): Promise<void> {
  const d = ev.data;
  const q = (sql: string, params: unknown[]) => db.query(sql, params.map(s));
  switch (`${ev.program}:${ev.name}`) {
    // ── auction-engine ────────────────────────────────────────────────
    case "auction-engine:PhaseChanged":
    case "auction-engine:SessionFailed": {
      const to = ev.name === "SessionFailed" ? "Failed" : variant(d.to);
      await q(
        `INSERT INTO sessions (session, phase, phase_slot, failed_reason) VALUES ($1, $2, $3, $4)
         ON CONFLICT (session) DO UPDATE SET
           phase = CASE WHEN sessions.phase = ANY($6::text[]) THEN sessions.phase
                        WHEN array_position($5::text[], excluded.phase) >= coalesce(array_position($5::text[], sessions.phase), 0)
                        THEN excluded.phase ELSE sessions.phase END,
           phase_slot = greatest(sessions.phase_slot, excluded.phase_slot),
           failed_reason = CASE WHEN sessions.phase = ANY($6::text[]) THEN sessions.failed_reason
                                ELSE coalesce(excluded.failed_reason, sessions.failed_reason) END`,
        [d.session, to, slot, ev.name === "SessionFailed" ? variant(d.reason) : null, `{${PHASES.join(",")}}`, `{${TERMINAL.join(",")}}`],
      );
      return;
    }
    case "auction-engine:Cleared":
      await q(
        `INSERT INTO sessions (session, clear_price, cleared_volume, reserve, reference_mark) VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (session) DO UPDATE SET clear_price = excluded.clear_price, cleared_volume = excluded.cleared_volume,
           reserve = excluded.reserve, reference_mark = excluded.reference_mark`,
        [d.session, d.price, d.volume, d.reserve, d.reference],
      );
      return;
    case "auction-engine:ClearingPrint":
      await q(
        `INSERT INTO sessions (session, asset_mint, print_price, print_volume, n_investor_ids, forced_sale) VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (session) DO UPDATE SET asset_mint = excluded.asset_mint, print_price = excluded.print_price,
           print_volume = excluded.print_volume, n_investor_ids = excluded.n_investor_ids, forced_sale = excluded.forced_sale`,
        [d.session, d.asset_mint, d.price, d.volume, d.n_investor_ids, d.forced_sale],
      );
      return;
    case "auction-engine:OrderCommitted":
      await q(
        `INSERT INTO bids (session, owner, side, commitment, escrowed) VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (session, owner) DO UPDATE SET side = excluded.side, commitment = excluded.commitment, escrowed = excluded.escrowed`,
        [d.session, d.owner, variant(d.side), hex(d.commitment), d.escrowed],
      );
      return;
    case "auction-engine:OrderRevealed":
      await q(
        `INSERT INTO bids (session, owner, side, revealed, limit_price, qty) VALUES ($1, $2, $3, true, $4, $5)
         ON CONFLICT (session, owner) DO UPDATE SET revealed = true, limit_price = excluded.limit_price, qty = excluded.qty`,
        [d.session, d.owner, variant(d.side), d.limit, d.qty],
      );
      return;
    case "auction-engine:FillAllocated":
      await q(
        `INSERT INTO fills (session, fill_index, buyer, seller, qty) VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (session, fill_index) DO UPDATE SET buyer = excluded.buyer, seller = excluded.seller, qty = excluded.qty`,
        [d.session, d.fill_index, d.buyer, d.seller, d.qty],
      );
      return;
    case "auction-engine:FillSettled":
      await q(
        `INSERT INTO fills (session, fill_index, buyer, seller, qty, price, settled, settle_signature) VALUES ($1, $2, $3, $4, $5, $6, true, $7)
         ON CONFLICT (session, fill_index) DO UPDATE SET price = excluded.price, settled = true, settle_signature = excluded.settle_signature`,
        [d.session, d.fill_index, d.buyer, d.seller, d.qty, d.price, sig],
      );
      return;
    // ── payment-rails ────────────────────────────────────────────────
    case "payment-rails:FillSettled": {
      // Behind an engine the rails session is not the auction session: the
      // engine's own FillSettled for the same fill in this transaction says
      // which session row the cash split belongs to.
      const engine = all.find(
        (e) => e.program === "auction-engine" && e.name === "FillSettled" && e.data.fill_index === d.fill_index && e.data.buyer === d.buyer,
      );
      const session = engine ? engine.data.session : d.session;
      await q(
        `INSERT INTO fills (session, fill_index, buyer, seller, qty, price, gross, to_seller, fee, to_reserve, rofr, settled, settle_signature)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, true, $12)
         ON CONFLICT (session, fill_index) DO UPDATE SET price = excluded.price, gross = excluded.gross, to_seller = excluded.to_seller,
           fee = excluded.fee, to_reserve = excluded.to_reserve, rofr = excluded.rofr, settled = true, settle_signature = excluded.settle_signature`,
        [session, d.fill_index, d.buyer, d.seller, d.qty, d.price, d.gross, d.to_seller, d.fee, d.to_reserve, d.rofr, sig],
      );
      return;
    }
    case "payment-rails:PayoutMade":
      await q(
        `INSERT INTO payouts (signature, event_index, kind, source, destination, amount) VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT DO NOTHING`,
        [sig, ev.index, variant(d.kind), d.source, d.destination, d.amount],
      );
      return;
    // ── credit-gate ──────────────────────────────────────────────────
    case "credit-gate:LotEvent":
      await q(
        `INSERT INTO lot_events (signature, event_index, slot, mint, owner, kind, lot_id, qty, restriction_expiry, free_qty)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) ON CONFLICT DO NOTHING`,
        [sig, ev.index, slot, d.mint, d.owner, LOT_KIND[d.kind as number] ?? String(d.kind), d.lot_id, d.qty, d.restriction_expiry, d.free_qty],
      );
      return;
    case "credit-gate:SettleChecked":
      // The seller's debit. The buyer's new lot arrives as its own LotEvent (append).
      await q(
        `INSERT INTO lot_events (signature, event_index, slot, mint, owner, kind, qty, free_qty)
         VALUES ($1, $2, $3, $4, $5, 'sale', $6, $7) ON CONFLICT DO NOTHING`,
        [sig, ev.index, slot, d.mint, d.seller, d.qty, d.seller_free_qty],
      );
      return;
    case "credit-gate:ValidateDecision":
      await q(
        `INSERT INTO gate_decisions (signature, event_index, slot, mint, owner, reject, distinct_investors)
         VALUES ($1, $2, $3, $4, $5, $6, $7) ON CONFLICT DO NOTHING`,
        [sig, ev.index, slot, d.mint, d.owner, d.reject === null ? null : variant(d.reject), d.distinct_investors],
      );
      return;
    // ── mark-engine ──────────────────────────────────────────────────
    case "mark-engine:NavPublished":
      await q(
        `INSERT INTO marks (feed, computed_ts, nav_pico, effective_ts, model_version, inputs_hash, admin_drill, signature)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) ON CONFLICT DO NOTHING`,
        [d.feed, d.computed_ts, d.nav_pico, d.effective_ts, d.model_version, hex(d.inputs_hash), d.admin_drill, sig],
      );
      return;
    case "mark-engine:MarkStateChanged":
      await q(
        `INSERT INTO mark_states (feed, at, from_state, to_state, breaker) VALUES ($1, $2, $3, $4, $5) ON CONFLICT DO NOTHING`,
        [d.feed, d.at, variant(d.from), variant(d.to), variant(d.breaker)],
      );
      return;
    case "mark-engine:PrintRecorded":
      await q(
        `INSERT INTO prints (feed, session, price, volume, n_investor_ids, forced_sale, eligibility, implied_pico, signature)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) ON CONFLICT DO NOTHING`,
        [d.feed, d.session, d.price, d.volume, d.n_investor_ids, d.forced_sale, variant(d.eligibility), d.implied_pico, sig],
      );
      return;
    // ── queue-claim ──────────────────────────────────────────────────
    case "queue-claim:RequestSubmitted":
      await q(
        `INSERT INTO claims (request, holder, qty, window_date, state, state_slot) VALUES ($1, $2, $3, $4, 'Requested', $5)
         ON CONFLICT (request) DO UPDATE SET holder = excluded.holder, qty = excluded.qty, window_date = excluded.window_date`,
        [d.request, d.holder, d.qty, d.window_date, slot],
      );
      return;
    case "queue-claim:RequestAttested":
      await q(
        `INSERT INTO claims (request, rank, claim_mint) VALUES ($1, $2, $3)
         ON CONFLICT (request) DO UPDATE SET rank = excluded.rank, claim_mint = excluded.claim_mint`,
        [d.request, d.rank, d.claim_mint],
      );
      return;
    case "queue-claim:StateChanged":
    case "queue-claim:FillRecorded":
      await q(
        `INSERT INTO claims (request, state, state_slot, filled_qty) VALUES ($1, $2, $3, $4)
         ON CONFLICT (request) DO UPDATE SET
           state = CASE WHEN claims.state_slot IS NULL OR excluded.state_slot >= claims.state_slot THEN excluded.state ELSE claims.state END,
           state_slot = greatest(claims.state_slot, excluded.state_slot),
           filled_qty = coalesce(excluded.filled_qty, claims.filled_qty)`,
        [d.request, variant(d.state), slot, ev.name === "FillRecorded" ? d.filled_qty : null],
      );
      return;
    case "queue-claim:FlagsSet":
      await q(
        `INSERT INTO claims (request, transferable, pledgeable) VALUES ($1, $2, $3)
         ON CONFLICT (request) DO UPDATE SET transferable = excluded.transferable, pledgeable = excluded.pledgeable`,
        [d.request, d.transferable, d.pledgeable],
      );
      return;
    case "queue-claim:ClaimPledged":
      await q(
        `INSERT INTO claims (request, pledged_facility) VALUES ($1, $2) ON CONFLICT (request) DO UPDATE SET pledged_facility = excluded.pledged_facility`,
        [d.request, d.facility],
      );
      return;
    case "queue-claim:ClaimPv":
      await q(`INSERT INTO claims (request, last_pv) VALUES ($1, $2) ON CONFLICT (request) DO UPDATE SET last_pv = excluded.last_pv`, [
        d.request,
        d.pv,
      ]);
      return;
    // ── liquidity-facility ───────────────────────────────────────────
    case "liquidity-facility:Pledged":
      await q(
        `INSERT INTO advances (position, borrower, qty, collateral_value, advanced, state, state_slot) VALUES ($1, $2, $3, $4, $5, 'Active', $6)
         ON CONFLICT (position) DO UPDATE SET borrower = excluded.borrower, qty = excluded.qty,
           collateral_value = excluded.collateral_value, advanced = excluded.advanced`,
        [d.position, d.borrower, d.qty, d.collateral_value, d.advanced, slot],
      );
      return;
    case "liquidity-facility:CollateralAdded":
      await q(`INSERT INTO advances (position, qty) VALUES ($1, $2) ON CONFLICT (position) DO UPDATE SET qty = excluded.qty`, [d.position, d.total]);
      return;
    case "liquidity-facility:HealthChecked":
    case "liquidity-facility:LadderStep": {
      const state = ev.name === "LadderStep" ? variant(d.to) : variant(d.state);
      await q(
        `INSERT INTO advances (position, state, state_slot, last_ltv_bps) VALUES ($1, $2, $3, $4)
         ON CONFLICT (position) DO UPDATE SET
           state = CASE WHEN advances.state_slot IS NULL OR excluded.state_slot >= advances.state_slot THEN excluded.state ELSE advances.state END,
           state_slot = greatest(advances.state_slot, excluded.state_slot),
           last_ltv_bps = coalesce(excluded.last_ltv_bps, advances.last_ltv_bps)`,
        [d.position, state, slot, ev.name === "HealthChecked" ? d.ltv_bps : null],
      );
      return;
    }
    case "liquidity-facility:ForcedSaleSettled":
    case "liquidity-facility:LadderSale":
      await q(`INSERT INTO advances (position, shortfall) VALUES ($1, $2) ON CONFLICT (position) DO UPDATE SET shortfall = excluded.shortfall`, [
        d.position,
        d.shortfall,
      ]);
      return;
    case "liquidity-facility:Released":
      await q(`INSERT INTO advances (position, released) VALUES ($1, true) ON CONFLICT (position) DO UPDATE SET released = true`, [d.position]);
      return;
    // ── tranche-engine ───────────────────────────────────────────────
    case "tranche-engine:PeriodDistributed":
      await q(
        `INSERT INTO waterfall_lines (pool, period, cash, fees, senior_coupon, reserve, senior_amort, junior_coupon, junior_residual, unpaid_senior_coupon, signature)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) ON CONFLICT DO NOTHING`,
        [d.pool, d.period, d.cash, d.fees, d.senior_coupon, d.reserve, d.senior_amort, d.junior_coupon, d.junior_residual, d.unpaid_senior_coupon, sig],
      );
      return;
    default:
      // Everything else is kept in `events` only.
      return;
  }
}

// ── RPC backfill ────────────────────────────────────────────────────────

export async function fetchTx(rpc: R, signature: string): Promise<RawTx | null> {
  const t = await rpc
    .getTransaction(signature as Signature, { encoding: "json", maxSupportedTransactionVersion: 0, commitment: "confirmed" })
    .send();
  if (!t) return null;
  return { signature, slot: t.slot, blockTime: t.blockTime, err: t.meta?.err ?? null, logs: t.meta?.logMessages ?? [] };
}

/**
 * Ingest everything `program` has done since the stored cursor, oldest
 * first. The cursor moves after each transaction, so an interrupted run
 * resumes where it stopped.
 */
export async function backfillProgram(rpc: R, db: Db, program: Address, opts: { pageSize?: number } = {}): Promise<{ txs: number; events: number }> {
  const [cur] = await db.query<{ last_signature: string | null }>(`SELECT last_signature FROM cursors WHERE source = $1`, [program]);
  const until = cur?.last_signature ?? undefined;
  const sigs: { signature: string; slot: bigint }[] = [];
  let before: string | undefined;
  for (;;) {
    const page = await rpc
      .getSignaturesForAddress(program, {
        limit: opts.pageSize ?? 1000,
        commitment: "confirmed",
        ...(before ? { before: before as Signature } : {}),
        ...(until ? { until: until as Signature } : {}),
      })
      .send();
    sigs.push(...page.map((p) => ({ signature: p.signature as string, slot: p.slot })));
    if (page.length < (opts.pageSize ?? 1000)) break;
    before = page[page.length - 1]!.signature;
  }
  let events = 0;
  for (const { signature, slot } of sigs.reverse()) {
    const tx = await fetchTx(rpc, signature);
    if (tx) events += await ingest(db, tx);
    await db.query(
      `INSERT INTO cursors (source, last_signature, last_slot, updated_at) VALUES ($1, $2, $3, now())
       ON CONFLICT (source) DO UPDATE SET last_signature = excluded.last_signature, last_slot = excluded.last_slot, updated_at = now()`,
      [program, signature, slot.toString()],
    );
  }
  return { txs: sigs.length, events };
}

/** Backfill every indexed program, or only `only` (names, e.g. the ones deployed on a cluster). */
export async function backfillAll(rpc: R, db: Db, only?: string[]): Promise<Record<string, { txs: number; events: number }>> {
  const out: Record<string, { txs: number; events: number }> = {};
  for (const [id, p] of BY_ID) {
    if (only && !only.includes(p.name)) continue;
    out[p.name] = await backfillProgram(rpc, db, id);
  }
  return out;
}

// ── webhooks ────────────────────────────────────────────────────────────

/** Helius raw webhook item (also what `getTransaction` returns, plus the signature list). */
interface WebhookTx {
  slot: number;
  blockTime?: number | null;
  meta?: { err?: unknown; logMessages?: string[] | null } | null;
  transaction?: { signatures?: string[] };
}

export function parseWebhook(body: unknown): RawTx[] {
  const items = Array.isArray(body) ? body : [body];
  return (items as WebhookTx[]).flatMap((t) => {
    const signature = t.transaction?.signatures?.[0];
    if (!signature || typeof t.slot !== "number") return [];
    return [{ signature, slot: t.slot, blockTime: t.blockTime ?? null, err: t.meta?.err ?? null, logs: t.meta?.logMessages ?? [] }];
  });
}

/** Largest webhook body accepted (Helius batches up to 100 txs; logs are capped per tx). */
export const MAX_WEBHOOK_BYTES = 4 * 1024 * 1024;

/**
 * POST handler for raw-transaction webhooks. `secret` must equal the
 * request's `Authorization` header (Helius sends the configured value
 * verbatim). Replies 200 with what was ingested, 413 over
 * `MAX_WEBHOOK_BYTES`, 500 if the database write fails (Helius retries;
 * ingest is idempotent, so a partial batch is safe to resend).
 */
export function webhookHandler(db: Db, secret: string, maxBytes = MAX_WEBHOOK_BYTES) {
  return async (req: IncomingMessage, res: ServerResponse) => {
    if (req.method !== "POST") return void res.writeHead(405).end();
    if (!secret || req.headers.authorization !== secret) return void res.writeHead(401).end();
    if (Number(req.headers["content-length"] ?? 0) > maxBytes) return void res.writeHead(413, { connection: "close" }).end();
    const chunks: Buffer[] = [];
    let size = 0;
    for await (const c of req) {
      size += (c as Buffer).length;
      if (size > maxBytes) return void res.writeHead(413, { connection: "close" }).end();
      chunks.push(c as Buffer);
    }
    let txs: RawTx[];
    try {
      txs = parseWebhook(JSON.parse(Buffer.concat(chunks).toString("utf8")));
    } catch {
      return void res.writeHead(400).end("bad json");
    }
    try {
      let events = 0;
      for (const tx of txs) events += await ingest(db, tx);
      res.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify({ txs: txs.length, events }));
    } catch (e) {
      console.error("webhook ingest failed:", e instanceof Error ? e.message : e);
      res.writeHead(500).end("ingest failed");
    }
  };
}

export type { Struct };
