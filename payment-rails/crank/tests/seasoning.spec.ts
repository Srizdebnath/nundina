// #127 seasoning crank: the HolderLots decoder agrees with credit-gate's
// pinned layout (lots.rs layout_read_by_the_seasoning_crank).
import assert from "node:assert/strict";
import { test } from "node:test";
import { HOLDER_LOTS_LEN, hasSeasonedLot } from "../src/seasoning.ts";

function ledger(expiries: bigint[]): Uint8Array {
  const b = new Uint8Array(HOLDER_LOTS_LEN);
  const v = new DataView(b.buffer);
  b[84] = expiries.length;
  expiries.forEach((e, i) => v.setBigInt64(85 + 31 * i + 20, e, true));
  return b;
}

test("a ledger is due only once one of its live lots has seasoned", () => {
  assert.equal(hasSeasonedLot(ledger([]), 1_000n), false);
  assert.equal(hasSeasonedLot(ledger([2_000n, 3_000n]), 1_999n), false);
  assert.equal(hasSeasonedLot(ledger([2_000n, 3_000n]), 2_000n), true, "expiry is inclusive, like merge_expired");
  assert.equal(hasSeasonedLot(ledger([5_000n, 1_000n]), 1_500n), true, "any lot, not just the first");
});

test("slots past lot_count are ignored, and other account sizes are not ledgers", () => {
  const b = ledger([9_000n]);
  new DataView(b.buffer).setBigInt64(85 + 31 + 20, 1n, true); // stale slot 1
  assert.equal(hasSeasonedLot(b, 5_000n), false);
  assert.equal(hasSeasonedLot(new Uint8Array(100), 5_000n), false);
});
