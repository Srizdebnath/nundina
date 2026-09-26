/**
 * Same truth table as `state.rs` tests (`open_session_refund_table`,
 * `failed_session_refunds_immediately`, `settled_session_…`): if the Rust
 * rules change, this mirror must change with them.
 */
import assert from "node:assert/strict";
import { test } from "node:test";
import { bondForfeit, refundRoute, type SessionStatus } from "../src/rules.ts";

const s = (status: SessionStatus, failedAt = 0n) => ({ status, revealEnd: 200n, settleDeadline: 300n, failedAt });

test("open session: locked, then unrevealed forfeits, revealed waits for deadline", () => {
  for (const now of [0n, 99n, 100n, 199n]) {
    assert.equal(refundRoute(s("Open"), false, now), null);
    assert.equal(refundRoute(s("Open"), true, now), null);
  }
  assert.equal(refundRoute(s("Open"), false, 200n), "ToReserve");
  assert.equal(refundRoute(s("Open"), true, 299n), null);
  assert.equal(refundRoute(s("Open"), true, 300n), "ToOwner");
});

test("failed before reveals close: everyone whole; after: non-revealers forfeit", () => {
  for (const failedAt of [50n, 150n, 199n]) {
    for (const now of [failedAt, 199n, 250n, 400n]) {
      assert.equal(refundRoute(s("Failed", failedAt), false, now), "ToOwner");
      assert.equal(refundRoute(s("Failed", failedAt), true, now), "ToOwner");
    }
  }
  assert.equal(refundRoute(s("Failed", 250n), false, 250n), "ToReserve");
  assert.equal(refundRoute(s("Failed", 250n), true, 250n), "ToOwner");
});

test("settled: everything refundable at once, unrevealed bonds still forfeit", () => {
  assert.equal(refundRoute(s("Settled"), true, 250n), "ToOwner");
  assert.equal(refundRoute(s("Settled"), false, 250n), "ToReserve");
});

test("revealers never forfeit", () => {
  for (const st of ["Open", "Failed", "Settled"] as const)
    for (const now of [0n, 200n, 300n, 10n ** 12n]) assert.equal(bondForfeit(s(st, 250n), true, now), false);
});
