// The publisher's arithmetic equals credit-math's (programs/credit-math/src/nav.rs):
// the crate's own hand vectors, its floor-against-the-publisher rounding,
// and a reproducible inputs hash.
import assert from "node:assert/strict";
import { test } from "node:test";
import { canonical, feeAccrualPerPeriod, inputsHash, navGrossPico, weeklyPublish, type NavInputs } from "../src/nav.ts";
import { publishNavIx, u128le } from "../src/nav-publisher.ts";
import { address } from "@solana/kit";

test("credit-math vectors: gross, weekly fee, published", () => {
  // nav_gross_hand_vector_pilot: $3,001.234567 sleeve / 3,000 NPCS.
  assert.equal(navGrossPico(3_001_234_567n, 3_000_000_000n), 1_000_411_522_333n);
  // fee_exact_at_unit_nav: 1e12 × 10 bps × 7 days → 19_178_082 (floored from …082.19).
  assert.equal(feeAccrualPerPeriod(1_000_000_000_000n, 10n, 604_800n), 19_178_082n);
  assert.equal(feeAccrualPerPeriod(1_000_000_000_000n, 10n, 0n), 0n);
  // weekly_publish_chain.
  const r = weeklyPublish(3_001_234_567n, 3_000_000_000n, 10n, 604_800n);
  assert.equal(r.gross, 1_000_411_522_333n);
  assert.equal(r.fee, 19_185_974n);
  assert.equal(r.published, r.gross - r.fee);
  // 1 USDC behind 1 NPCS (devnet today) is NAV_ONE.
  assert.equal(navGrossPico(1_000_000n, 1_000_000n), 1_000_000_000_000n);
});

test("zero supply has no NAV", () => {
  assert.throws(() => navGrossPico(1n, 0n), /supply is zero/);
});

test("inputs hash is over sorted-key JSON and moves with any input", () => {
  const inputs: NavInputs = {
    model_version: 1,
    asset_mint: "6vt1wHVQVuhnfJ3FtNbq4647mCjWUBmJCDABsYUgVKL4",
    supply_base: "1000000",
    treasury_source: "sleeve",
    treasury_account: "B78WTdsY4MYhAvT757xtdb6KZ61pkBTPzKCtohK3NKAz",
    treasury_micros: "1000000",
    slot: "1",
    fee_bps: "10",
    delta_seconds: "0",
    previous_computed_ts: "0",
    computed_ts: "0",
  };
  const c = canonical(inputs);
  assert.ok(c.startsWith('{"asset_mint":'));
  const h = Buffer.from(inputsHash(inputs)).toString("hex");
  assert.equal(h, Buffer.from(inputsHash({ ...inputs })).toString("hex"));
  assert.notEqual(h, Buffer.from(inputsHash({ ...inputs, treasury_micros: "1000001" })).toString("hex"));
});

test("publish_nav data: disc | u128 nav | i64 computed_ts | u32 model | [32] hash", () => {
  assert.deepEqual([...u128le((1n << 64n) + 2n)], [2, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0]);
  const feed = address("79ypVjMMCDbmxYWaXTTgxz9jidjncoJAhHQC6e13x4LH");
  const ix = publishNavIx(feed, feed, {
    published: 1_000_000_000_000n,
    inputs: { computed_ts: "1700000000", model_version: 1 } as NavInputs,
    hash: new Uint8Array(32).fill(7),
  });
  assert.equal(ix.data!.length, 8 + 16 + 8 + 4 + 32);
  assert.equal(new DataView(ix.data!.buffer).getBigInt64(24, true), 1_700_000_000n);
  assert.equal(new DataView(ix.data!.buffer).getUint32(32, true), 1);
});
