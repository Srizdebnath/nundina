// The indexer against real transactions: a local demo run (validator with
// the real programs; app/scripts/localnet.sh + localnet:e2e + a crank-runner
// tick) captured by test/capture-localnet.ts. PGlite = real Postgres.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createServer } from "node:http";
import type { AddressInfo } from "node:net";
import { after, before, test } from "node:test";
import { apiServer } from "../src/api.ts";
import { openDb, type Db } from "../src/db.ts";
import { ingest, parseWebhook, webhookHandler, type RawTx } from "../src/indexer.ts";
import { eventsFromLogs } from "../src/logs.ts";
import { programId } from "../src/programs.ts";
import { address, getAddressEncoder } from "@solana/kit";

const TXS = JSON.parse(readFileSync(new URL("./fixtures/localnet-txs.json", import.meta.url), "utf8")) as RawTx[];
let db: Db;

const one = async <T = Record<string, unknown>>(sql: string, p: unknown[] = []) => (await db.query<T>(sql, p))[0]!;

before(async () => {
  db = await openDb("pglite:memory");
  for (const tx of TXS) await ingest(db, tx);
});
after(async () => db.close());

test("fixture is a real run: a printed session, a paid coupon period", () => {
  const names = new Set(TXS.flatMap((t) => eventsFromLogs(t.logs).map((e) => `${e.program}:${e.name}`)));
  for (const n of ["auction-engine:Cleared", "payment-rails:FillSettled", "credit-gate:SettleChecked", "mark-engine:PrintRecorded", "mark-engine:NavPublished", "tranche-engine:PeriodDistributed"]) {
    assert.ok(names.has(n), `missing ${n}`);
  }
});

test("FillSettled is attributed by the invoke stack, not the discriminator", () => {
  const settle = TXS.find((t) => eventsFromLogs(t.logs).some((e) => e.program === "payment-rails" && e.name === "FillSettled"))!;
  const evs = eventsFromLogs(settle.logs).filter((e) => e.name === "FillSettled");
  assert.deepEqual(evs.map((e) => e.program).sort(), ["auction-engine", "payment-rails"]);
  // Same discriminator, different layouts: only the right codec decodes each.
  assert.ok("gross" in evs.find((e) => e.program === "payment-rails")!.data);
  assert.ok(!("gross" in evs.find((e) => e.program === "auction-engine")!.data));
});

test("session, bids, fills and print are projected", async () => {
  const s = await one<{ session: string; phase: string; clear_price: string; cleared_volume: string; print_price: string }>(
    `SELECT * FROM sessions WHERE clear_price IS NOT NULL`,
  );
  assert.equal(s.phase, "Printed");
  assert.equal(s.clear_price, "945000");
  assert.equal(s.cleared_volume, "8000000");
  assert.equal(s.print_price, "945000");
  const bids = await db.query<{ revealed: boolean }>(`SELECT * FROM bids WHERE session = $1`, [s.session]);
  assert.equal(bids.length, 5);
  assert.ok(bids.every((b) => b.revealed));
  const fills = await db.query<{ qty: string; gross: string; settled: boolean }>(`SELECT * FROM fills WHERE session = $1 ORDER BY fill_index`, [s.session]);
  assert.equal(fills.length, 2);
  // The rails cash split lands on the auction session's rows, not on the rails session.
  assert.ok(fills.every((f) => f.settled && f.gross !== null));
  assert.equal(fills.reduce((a, f) => a + BigInt(f.qty), 0n), 8_000_000n);
  const print = await one<{ eligibility: string }>(`SELECT * FROM prints WHERE session = $1`, [s.session]);
  assert.equal(print.eligibility, "Eligible");
});

test("NAV publication is indexed with its inputs hash", async () => {
  const m = await one<{ nav_pico: string; model_version: number; inputs_hash: string; admin_drill: boolean }>(`SELECT * FROM marks ORDER BY computed_ts DESC LIMIT 1`);
  assert.equal(m.model_version, 1);
  assert.equal(m.admin_drill, false);
  assert.match(m.inputs_hash, /^[0-9a-f]{64}$/);
  // 130 NPCS backed by 130 USDC, minus a few seconds of the 10 bps/yr fee.
  assert.ok(BigInt(m.nav_pico) < 1_000_000_000_000n && BigInt(m.nav_pico) > 999_990_000_000n);
});

test("waterfall line conserves the period's cash", async () => {
  const w = await one<Record<string, string>>(`SELECT * FROM waterfall_lines ORDER BY period LIMIT 1`);
  const parts = ["fees", "senior_coupon", "reserve", "senior_amort", "junior_coupon", "junior_residual"].reduce((a, k) => a + BigInt(w[k]!), 0n);
  assert.equal(parts, BigInt(w.cash!));
});

test("lot ledger: the buyer's lot appears once, the seller's sale once", async () => {
  const buys = await db.query(`SELECT * FROM lot_events WHERE kind = 'append'`);
  const sales = await db.query(`SELECT * FROM lot_events WHERE kind = 'sale'`);
  const settles = TXS.flatMap((t) => eventsFromLogs(t.logs)).filter((e) => e.name === "SettleChecked").length;
  assert.equal(sales.length, settles);
  assert.ok(buys.length >= settles);
});

test("re-ingesting everything (backfill overlapping a webhook) changes nothing", async () => {
  const before = await db.query(`SELECT (SELECT count(*) FROM events) e, (SELECT count(*) FROM fills) f, (SELECT count(*) FROM lot_events) l`);
  for (const tx of [...TXS].reverse()) await ingest(db, tx);
  const after = await db.query(`SELECT (SELECT count(*) FROM events) e, (SELECT count(*) FROM fills) f, (SELECT count(*) FROM lot_events) l`);
  assert.deepEqual(after, before);
  // Reverse order must not move the phase backwards.
  assert.equal((await one<{ phase: string }>(`SELECT phase FROM sessions WHERE clear_price IS NOT NULL`)).phase, "Printed");
});

test("an event that does not fit the IDL is kept raw, not dropped, not projected", async () => {
  const d = await openDb("pglite:memory");
  const RAILS = "GdWmRivxe1k76apQB7EgJ4yBq8Fi3Lw25E8V3XosPocn";
  // A FillSettled discriminator with a body too short for the layout.
  const settle = TXS.flatMap((t) => t.logs).find((l) => l.startsWith("Program data: ") && eventsFromLogs([`Program ${RAILS} invoke [1]`, l]).some((e) => e.name === "FillSettled"))!;
  const short = Buffer.from(settle.slice(14), "base64").subarray(0, 20).toString("base64");
  await ingest(d, { signature: "x", slot: 1, blockTime: null, err: null, logs: [`Program ${RAILS} invoke [1]`, `Program data: ${short}`, `Program ${RAILS} success`] });
  const [row] = await d.query<{ name: string; data: { undecoded: boolean; raw: string } }>(`SELECT name, data FROM events`);
  assert.equal(row!.name, "FillSettled");
  assert.equal(row!.data.undecoded, true);
  assert.equal(row!.data.raw, short);
  assert.equal((await d.query(`SELECT * FROM fills`)).length, 0);
  await d.close();
});

test("failed transactions are not indexed", async () => {
  const d = await openDb("pglite:memory");
  const ok = TXS.find((t) => eventsFromLogs(t.logs).length > 0)!;
  assert.equal(await ingest(d, { ...ok, err: { InstructionError: [0, { Custom: 1 }] } }), 0);
  assert.equal((await d.query<{ n: number }>(`SELECT count(*)::int n FROM events`))[0]!.n, 0);
  await d.close();
});

test("webhook: Helius raw shape, authorization required", async () => {
  const t = TXS[0]!;
  const body = [{ slot: t.slot, blockTime: t.blockTime, meta: { err: null, logMessages: t.logs }, transaction: { signatures: [t.signature] } }];
  assert.deepEqual(parseWebhook(body)[0]!.signature, t.signature);
  const d = await openDb("pglite:memory");
  const h = webhookHandler(d, "s3cret");
  const srv = createServer((req, res) => void h(req, res)).listen(0);
  const url = `http://127.0.0.1:${(srv.address() as AddressInfo).port}/`;
  assert.equal((await fetch(url, { method: "POST", body: JSON.stringify(body) })).status, 401);
  const r = await fetch(url, { method: "POST", headers: { authorization: "s3cret" }, body: JSON.stringify(body) });
  assert.equal(r.status, 200);
  assert.equal(((await r.json()) as { txs: number }).txs, 1);
  srv.close();
  await d.close();
});

test("webhook: oversized body is 413, a database error is 500 (no unhandled rejection)", async () => {
  const t = TXS[0]!;
  const body = JSON.stringify([{ slot: t.slot, blockTime: t.blockTime, meta: { err: null, logMessages: t.logs }, transaction: { signatures: [t.signature] } }]);
  const d = await openDb("pglite:memory");
  const small = webhookHandler(d, "s3cret", 64);
  const broken = webhookHandler({ ...d, query: async () => { throw new Error("db down"); } } as Db, "s3cret");
  const serve = (h: typeof small) => {
    const srv = createServer((req, res) => void h(req, res)).listen(0);
    return { srv, url: `http://127.0.0.1:${(srv.address() as AddressInfo).port}/` };
  };
  const a = serve(small);
  assert.equal((await fetch(a.url, { method: "POST", headers: { authorization: "s3cret" }, body })).status, 413);
  a.srv.close();
  const b = serve(broken);
  const logged = console.error;
  console.error = () => {};
  try {
    // No try/catch → no response at all: the timeout turns that into a failure, not a hang.
    assert.equal((await fetch(b.url, { method: "POST", headers: { authorization: "s3cret" }, body, signal: AbortSignal.timeout(5_000) })).status, 500);
  } finally {
    console.error = logged;
  }
  b.srv.close();
  await d.close();
});

test("a late SessionFailed never overwrites a terminal Printed", async () => {
  const s = await one<{ session: string; phase: string }>(`SELECT session, phase FROM sessions WHERE phase = 'Printed'`);
  const AUCTION = programId("auction-engine");
  const data = Buffer.concat([
    Buffer.from([103, 232, 4, 103, 254, 254, 62, 253]), // SessionFailed
    Buffer.from(getAddressEncoder().encode(address(s.session))),
    Buffer.from([3]), // FailReason::Deadline
  ]).toString("base64");
  const n = await ingest(db, { signature: "late-failed", slot: 10n ** 12n, blockTime: null, err: null, logs: [`Program ${AUCTION} invoke [1]`, `Program data: ${data}`, `Program ${AUCTION} success`] });
  assert.equal(n, 1);
  const row = await one<{ phase: string; failed_reason: string | null }>(`SELECT phase, failed_reason FROM sessions WHERE session = $1`, [s.session]);
  assert.deepEqual(row, { phase: "Printed", failed_reason: null });
});

test("api serves the projections read-only", async () => {
  const srv = apiServer(db).listen(0);
  const base = `http://127.0.0.1:${(srv.address() as AddressInfo).port}`;
  const s = await one<{ session: string }>(`SELECT session FROM sessions WHERE clear_price IS NOT NULL`);
  const got = (await (await fetch(`${base}/sessions/${s.session}`)).json()) as { fills: unknown[]; bids: unknown[]; events: unknown[] };
  assert.equal(got.fills.length, 2);
  assert.equal(got.bids.length, 5);
  assert.ok(got.events.length > 0);
  assert.equal((await fetch(`${base}/sessions/${s.session}`, { method: "POST" })).status, 405);
  assert.equal((await fetch(`${base}/sessions/11111111111111111111111111111111`)).status, 404);
  const ev = (await (await fetch(`${base}/events?program=tranche-engine&name=PeriodDistributed`)).json()) as unknown[];
  assert.ok(ev.length >= 1);
  srv.close();
});
