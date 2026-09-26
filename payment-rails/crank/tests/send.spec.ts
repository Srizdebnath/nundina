import assert from "node:assert/strict";
import { test } from "node:test";
import { classify, DEFAULT_FEE, pickPriorityFee } from "../src/send.ts";

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
