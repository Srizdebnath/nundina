import assert from "node:assert/strict";
import { test } from "node:test";
import { backoffDelayMs, classify, DEFAULT_FEE, escalatedFee, pickPriorityFee } from "../src/send.ts";

test("priority fee: p75, clamped to [min, max], min when no data", () => {
  const cfg = { ...DEFAULT_FEE, minMicroLamports: 10n, maxMicroLamports: 1_000n };
  assert.equal(pickPriorityFee([], cfg), 10n);
  assert.equal(pickPriorityFee([0n, 0n, 0n, 0n], cfg), 10n);
  assert.equal(pickPriorityFee([100n, 200n, 300n, 400n], cfg), 400n); // idx floor(0.75*4)=3
  assert.equal(pickPriorityFee([5n, 50n, 500n, 5_000n, 50_000n], cfg), 1_000n);
  assert.equal(pickPriorityFee([400n, 100n, 300n, 200n], cfg), 400n, "order-independent");
});

test("anchor program error → typed code + name, not retried", () => {
  const e = { message: "Transaction simulation failed", context: { logs: ["Program log: AnchorError … Error Number: 6011. Error Message: Payout destination is not allowlisted."] } };
  const c = classify(e);
  assert.equal(c.code, 6011);
  assert.equal(c.name, "DestinationNotAllowlisted");
  assert.equal(c.transient, false);
});

test("init on an existing receipt → alreadyInUse (idempotent), not retried", () => {
  const c = classify({ message: "simulation failed", cause: { logs: ["Allocate: account Address { … } already in use", "Program 11111111111111111111111111111111 failed: custom program error: 0x0"] } });
  assert.equal(c.alreadyInUse, true);
  assert.equal(c.transient, false);
});

test("expired blockhash / network errors → transient (retried)", () => {
  assert.equal(classify(new Error("The network has progressed past the last block for which this transaction could have been committed (block height exceeded)")).transient, true);
  assert.equal(classify(new Error("fetch failed", { cause: new Error("ECONNRESET") })).transient, true);
  assert.equal(classify({ message: "HTTP error (429): Too Many Requests" }).transient, true);
  assert.equal(classify(new Error("something unexpected")).transient, false);
});

test("D11: first attempt is the oracle pick; retries escalate ×1.5 from a floor, clamped", () => {
  const cfg = { ...DEFAULT_FEE, minMicroLamports: 0n, maxMicroLamports: 50_000n, retryFloorMicroLamports: 1_000n, jitterPct: 0 };
  assert.equal(escalatedFee(0n, 1, cfg, 0.5), 0n, "attempt 1 = oracle, no floor");
  assert.equal(escalatedFee(0n, 2, cfg, 0.5), 1_500n, "oracle 0 → floor 1000 × 1.5");
  assert.equal(escalatedFee(0n, 3, cfg, 0.5), 2_250n);
  assert.equal(escalatedFee(10_000n, 2, cfg, 0.5), 15_000n);
  assert.equal(escalatedFee(40_000n, 3, cfg, 0.5), 50_000n, "clamped to max");
  assert.equal(escalatedFee(10_000n, 20, cfg, 0.5), 50_000n, "never exceeds max");
  const fees = [1, 2, 3, 4, 5, 6].map((a) => escalatedFee(100n, a, cfg, 0.5));
  for (let i = 1; i < fees.length; i++) assert.ok(fees[i]! >= fees[i - 1]!, "monotone without jitter");
});

test("D11: jitter stays within ±jitterPct and still respects [min, max]", () => {
  const cfg = { ...DEFAULT_FEE, minMicroLamports: 0n, maxMicroLamports: 50_000n, retryFloorMicroLamports: 1_000n, jitterPct: 10 };
  const lo = escalatedFee(10_000n, 2, cfg, 0); // 15000 − 10%
  const hi = escalatedFee(10_000n, 2, cfg, 0.999999); // 15000 + ~10%
  assert.equal(lo, 13_500n);
  assert.ok(hi > 16_490n && hi <= 16_500n, `hi=${hi}`);
  assert.ok(escalatedFee(49_000n, 3, cfg, 0.999999) <= 50_000n);
  assert.equal(escalatedFee(0n, 1, { ...cfg, minMicroLamports: 7n }, 0), 7n);
});

test("backoff: exponential with bounded jitter", () => {
  assert.equal(backoffDelayMs(500, 1, 0, 0.3), 500);
  assert.equal(backoffDelayMs(500, 3, 0, 0.3), 2_000);
  assert.equal(backoffDelayMs(500, 3, 25, 0), 1_500);
  assert.equal(backoffDelayMs(500, 3, 25, 0.5), 2_000);
  const d = backoffDelayMs(500, 3, 25, 0.9999);
  assert.ok(d > 2_490 && d <= 2_500);
});
