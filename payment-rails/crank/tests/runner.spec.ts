// #141: the crank-runner's schedule, the distribution crank's planning, the
// session evidence bundle, and the codec types they need (u128, Option).
// Logs are real: a local demo run captured by services/test/capture-localnet.ts.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { generateKeyPairSigner } from "@solana/kit";
import { distributionDue, railsPeriodId, type Pool } from "../src/distribute.ts";
import { signManifest, verifyEvidence, type UnsignedManifest } from "../src/evidence.ts";
import { loadIdl, makeCodec } from "../src/idl.ts";
import { killed, parseRunnerConfig } from "../src/runner.ts";
import { sessionEventsFromLogs } from "../src/session-evidence.ts";

const TXS = JSON.parse(
  readFileSync(new URL("../../../services/test/fixtures/localnet-txs.json", import.meta.url), "utf8"),
) as { signature: string; logs: string[] }[];

const pool = (state: string, last: bigint, periods = 3): Pool =>
  ({ pool_id: 1, state, periods, last_distribution: last, cash: "x", params: {} }) as unknown as Pool;

test("rails period id = pool id << 32 | period (tranche_engine::rails_period_id)", () => {
  assert.equal(railsPeriodId(1, 0), 1n << 32n);
  assert.equal(railsPeriodId(7, 5), (7n << 32n) | 5n);
});

test("a period is due only for Active/Amortizing pools, after the interval, at the pool's own counter", () => {
  assert.deepEqual(distributionDue(pool("Active", 100n), 200n, 100n), { period: 3 });
  assert.equal(distributionDue(pool("Active", 150n), 200n, 100n), null);
  assert.deepEqual(distributionDue(pool("Amortizing", 0n), 1n, 0n), { period: 3 });
  for (const s of ["Warehousing", "Standstill", "Impaired", "WoundDown"]) assert.equal(distributionDue(pool(s, 0n), 10n ** 9n, 0n), null);
});

test("runner config is validated before anything is sent", () => {
  const ok = parseRunnerConfig(
    JSON.stringify({ intervalSeconds: 30, seasoningMints: ["6vt1wHVQVuhnfJ3FtNbq4647mCjWUBmJCDABsYUgVKL4"], distribution: [{ pool: "CvcaeyuPu7SrFJ91WniJfBhUbQBkb8QQVL9fyK4wcFAS", cashPerPeriod: "1000000", minIntervalSeconds: 86400 }] }),
  );
  assert.equal(ok.intervalSeconds, 30);
  assert.throws(() => parseRunnerConfig(JSON.stringify({ intervalSeconds: 0 })), /intervalSeconds/);
  assert.throws(() => parseRunnerConfig(JSON.stringify({ intervalSeconds: 5, seasoningMints: ["nope"] })));
  assert.throws(
    () => parseRunnerConfig(JSON.stringify({ intervalSeconds: 5, distribution: [{ pool: "CvcaeyuPu7SrFJ91WniJfBhUbQBkb8QQVL9fyK4wcFAS", cashPerPeriod: "0", minIntervalSeconds: 0 }] })),
    /cashPerPeriod/,
  );
});

test("kill switch: the runner pauses all but refunds while CRANK_KILL_FILE exists (AC-11 drill)", () => {
  const dir = mkdtempSync(join(tmpdir(), "kill-"));
  const f = join(dir, "KILL");
  process.env.CRANK_KILL_FILE = f;
  assert.equal(killed(), false);
  writeFileSync(f, "");
  assert.equal(killed(), true);
  delete process.env.CRANK_KILL_FILE;
  assert.equal(killed(), false);
});

test("codec: u128 and Option round-trip (mark-engine NAVs, credit-gate rejections)", () => {
  const mark = makeCodec(loadIdl("mark_engine"));
  const big = (1n << 127n) + 12345n;
  assert.equal(mark.decodeAt("u128", mark.encode("u128", big), 0)[0], big);
  assert.deepEqual([...mark.encode({ option: "u64" }, null)], [0]);
  assert.equal(mark.decodeAt({ option: "u64" }, mark.encode({ option: "u64" }, 945_000n), 0)[0], 945_000n);
  assert.throws(() => mark.decodeAt({ option: "u64" }, Uint8Array.of(2, 0, 0, 0, 0, 0, 0, 0, 0), 0), /bad option tag/);
});

test("session bundle attribution on a real settle: both FillSettled, the gate's check and lot, each by its own program", () => {
  const settle = TXS.find((t) => sessionEventsFromLogs(t.logs).some((e) => e.program === "credit-gate" && e.name === "SettleChecked"))!;
  const evs = sessionEventsFromLogs(settle.logs);
  const fill = evs.filter((e) => e.name === "FillSettled");
  assert.deepEqual(fill.map((e) => e.program).sort(), ["auction-engine", "payment-rails"]);
  assert.ok("gross" in fill.find((e) => e.program === "payment-rails")!.data);
  assert.ok(evs.some((e) => e.program === "credit-gate" && e.name === "LotEvent"));
  const print = TXS.flatMap((t) => sessionEventsFromLogs(t.logs)).find((e) => e.name === "PrintRecorded")!;
  assert.equal(print.program, "mark-engine");
  assert.equal(print.data.price, 945_000n, "Option<u64> decoded");
});

test("verify-export: an edited snapshot is caught even though events.jsonl is intact", async () => {
  const dir = mkdtempSync(join(tmpdir(), "sess-"));
  const body = '{"event":"x"}\n';
  const snap = '{"feed":{"nav_pico":"1000000000000"}}\n';
  writeFileSync(join(dir, "events.jsonl"), body);
  writeFileSync(join(dir, "snapshot.json"), snap);
  const sha = (s: string) => createHash("sha256").update(s).digest("hex");
  const unsigned: UnsignedManifest = {
    program: "auction-engine session x",
    generatedAt: "2026-10-07T00:00:00Z",
    transactions: 1,
    events: 1,
    byEvent: { x: 1 },
    transferProof: { fills: 0, proven: 0, unproven: 0 },
    slotRange: ["1", "1"],
    file: "events.jsonl",
    sha256: sha(body),
    snapshot: { file: "snapshot.json", sha256: sha(snap) },
  };
  const signer = await generateKeyPairSigner();
  writeFileSync(join(dir, "manifest.json"), JSON.stringify(await signManifest(unsigned, signer)));
  assert.equal((await verifyEvidence(dir)).ok, true);
  writeFileSync(join(dir, "snapshot.json"), '{"feed":{"nav_pico":"2000000000000"}}\n');
  const r = await verifyEvidence(dir);
  assert.equal(r.ok, false);
  assert.match(r.problems.join(), /snapshot/);
});
