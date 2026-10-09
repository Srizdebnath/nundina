// #149 phase-deadline crank: the planner mirrors auction-engine's guards,
// and the session decoder reads the program's real IDL layout.
import assert from "node:assert/strict";
import { test } from "node:test";
import { address } from "@solana/kit";
import { auction, planPhase, type AuctionSession } from "../src/phases.ts";

const A = address("11111111111111111111111111111111");
const B = address("SysvarC1ock11111111111111111111111111111111");

type O = { side?: string; revealed?: boolean; screened?: boolean };
function session(phase: string, fills: { settled: boolean }[] = [], orders: O[] = []): AuctionSession {
  return {
    session_id: 1n,
    asset_mint: A,
    feed: A,
    rails_session: A,
    params: { reveal_end: 200n, rofr_window: 50n, settle_deadline: 400n },
    phase,
    clear_price: 0n,
    orders: orders.map((o) => ({ owner: B, side: o.side ?? "Buy", revealed: o.revealed ?? true, screened: o.screened ?? false })),
    fills: fills.map((f) => ({ buyer: A, seller: B, qty: 1n, rofr: false, settled: f.settled })),
  } as AuctionSession;
}

test("commit/reveal wait for reveal_end, then clear", () => {
  assert.equal(planPhase(session("Commit"), 199n), null);
  assert.deepEqual(planPhase(session("Reveal"), 200n), { kind: "clear" });
  assert.deepEqual(planPhase(session("Commit"), 250n), { kind: "clear" }, "nobody revealed: clear fails it");
});

test("AE-P6: every revealed order is screened, in book order, before the clear", () => {
  const book: O[] = [{ side: "Sell" }, { side: "Buy", revealed: false }, { side: "Buy" }];
  assert.equal(planPhase(session("Reveal", [], book), 199n), null, "reveals still open");
  assert.deepEqual(planPhase(session("Reveal", [], book), 200n), { kind: "screen", index: 0, side: "Sell" });
  const some: O[] = [{ side: "Sell", screened: true }, { side: "Buy", revealed: false }, { side: "Buy" }];
  assert.deepEqual(planPhase(session("Clearing", [], some), 210n), { kind: "screen", index: 2, side: "Buy" }, "unrevealed skipped");
  const all: O[] = [{ side: "Sell", screened: true }, { side: "Buy", revealed: false }, { side: "Buy", screened: true }];
  assert.deepEqual(planPhase(session("Clearing", [], all), 210n), { kind: "clear" });
  assert.deepEqual(planPhase(session("Clearing", [], some), 400n), { kind: "expire" }, "the deadline still wins");
});

test("allocate, then the ROFR window, then settle every fill, then finish", () => {
  assert.deepEqual(planPhase(session("Allocating"), 210n), { kind: "allocate" });
  assert.equal(planPhase(session("IssuerRofr"), 249n), null, "issuer still has its window");
  assert.deepEqual(planPhase(session("IssuerRofr"), 250n), { kind: "close_rofr" });
  const s = session("Settling", [{ settled: true }, { settled: false }, { settled: false }]);
  assert.deepEqual(planPhase(s, 260n), { kind: "settle", index: 1 });
  assert.deepEqual(planPhase(session("Settling", [{ settled: true }]), 260n), { kind: "finish" });
});

test("past the hard deadline every live session expires; terminal ones idle", () => {
  for (const p of ["Commit", "Reveal", "Clearing", "Allocating", "IssuerRofr", "Settling"]) {
    assert.deepEqual(planPhase(session(p), 400n), { kind: "expire" }, p);
  }
  assert.equal(planPhase(session("Printed"), 999n), null);
  assert.equal(planPhase(session("Failed"), 999n), null);
});

test("the session decoder round-trips the program's IDL layout", () => {
  const value = {
    venue: A,
    session_id: 7n,
    asset_mint: B,
    asset_decimals: 6,
    feed: A,
    rails_session: B,
    params: {
      commit_end: 100n,
      reveal_end: 200n,
      rofr_window: 50n,
      settle_deadline: 400n,
      bond: 5n,
      max_cash: 9n,
      min_reveals: 3,
      max_discount_bps: 1_000n,
      forced_sale: false,
      rofr_authority: A,
      disclosure_hash: new Uint8Array(32).fill(1),
    },
    phase: "Settling",
    reserve_price: 900_000n,
    reference_mark: 1_000_000n,
    clear_price: 950_000n,
    cleared_volume: 10n,
    rofr_qty: 2n,
    fills_settled: 1,
    orders: [
      {
        owner: B,
        side: "Sell",
        commitment: new Uint8Array(32).fill(2),
        escrowed: 10n,
        revealed: true,
        limit: 900_000n,
        qty: 10n,
        alloc: 10n,
        alloc_left: 0n,
        screened: true,
        eligible: true,
      },
    ],
    fills: [{ buyer: A, seller: B, qty: 8n, rofr: false, settled: true }],
    bump: 254,
  };
  const body = auction.encode({ defined: { name: "Session" } }, value);
  const data = new Uint8Array([...auction.accountDiscriminator("Session"), ...body]);
  const s = auction.decodeAccount<AuctionSession>("Session", data)!;
  assert.equal(s.phase, "Settling");
  assert.equal(s.clear_price, 950_000n);
  assert.equal(s.fills[0]!.qty, 8n);
  assert.equal(s.params.settle_deadline, 400n);
  assert.equal(s.orders[0]!.screened, true, "AE-P6 fields decode");
  assert.deepEqual(planPhase(s, 300n), { kind: "finish" });
});
