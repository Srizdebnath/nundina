import assert from "node:assert/strict";
import { test } from "node:test";
import { address } from "@solana/kit";
import { encode, idl, PROGRAM_ID, type Value } from "../src/idl.ts";
import { eventsFromLogs, toJson, transferProof } from "../src/evidence.ts";

const S = address("7yREa5V3ttjmwQ5dDv1DhThBzywgE5knJJuEAxppQa9c");
const TOKEN = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";

function sessionEventLine(action: string): string {
  const disc = idl.events.find((e) => e.name === "SessionEvent")!.discriminator;
  const body = encode({ defined: { name: "SessionEvent" } }, { action, session: S, operator: S, settle_deadline: 300n, ts: -1n });
  return "Program data: " + Buffer.from([...disc, ...body]).toString("base64");
}

test("events are attributed only to the rails program's own frame", () => {
  const logs = [
    `Program ${PROGRAM_ID} invoke [1]`,
    sessionEventLine("Opened"),
    `Program ${TOKEN} invoke [2]`,
    sessionEventLine("Failed"), // look-alike emitted inside a CPI'd program → ignored
    `Program ${TOKEN} success`,
    sessionEventLine("Settled"),
    `Program ${PROGRAM_ID} success`,
    sessionEventLine("Failed"), // outside any frame → ignored
  ];
  const evs = eventsFromLogs(logs);
  assert.deepEqual(evs.map((e) => [e.name, e.data.action]), [
    ["SessionEvent", "Opened"],
    ["SessionEvent", "Settled"],
  ]);
  assert.equal(evs[0]!.data.ts, -1n);
});

test("unknown payloads are skipped, not mis-decoded", () => {
  const logs = [`Program ${PROGRAM_ID} invoke [1]`, "Program data: AAAAAAAAAAA=", `Program ${PROGRAM_ID} success`];
  assert.deepEqual(eventsFromLogs(logs), []);
});

test("toJson: bigint → string, bytes → hex, nested", () => {
  assert.deepEqual(toJson({ a: 1n, b: new Uint8Array([0xab, 1]), c: [2n, { d: 3n }] }), { a: "1", b: "ab01", c: ["2", { d: "3" }] });
});

function transferLine(ev: Value): string {
  const disc = idl.events.find((e) => e.name === "TransferInstruction")!.discriminator;
  return "Program data: " + Buffer.from([...disc, ...encode({ defined: { name: "TransferInstruction" } }, ev)]).toString("base64");
}

test("transferProof: today's AE-P8 records prove nothing, and the manifest says so (#88)", () => {
  const base = { session: S, fill_index: 0, from: S, to: S, qty: 1n, price: 1n, disclosure_hash: new Uint8Array(32) };
  const today = { ...base, lot_ids: [], exemption_path: "NotRecorded", gate_check: "NotPerformed" };
  const lot = { lot_index: 0, acquired_ts: 0n, restriction_expiry: 0n };
  const proven = { ...base, fill_index: 1, lot_ids: [lot], exemption_path: "Section4a7", gate_check: "Passed" };
  // Each missing element alone keeps a fill unproven.
  const noLots = { ...proven, fill_index: 2, lot_ids: [] };
  const noBasis = { ...proven, fill_index: 3, exemption_path: "NotRecorded" };
  const noCheck = { ...proven, fill_index: 4, gate_check: "NotPerformed" };
  const logs = [`Program ${PROGRAM_ID} invoke [1]`, ...[today, proven, noLots, noBasis, noCheck].map(transferLine), `Program ${PROGRAM_ID} success`];
  // Decoded and JSON-shaped exactly as exportEvidence writes them.
  const records = eventsFromLogs(logs).map((e) => ({ event: e.name, data: toJson(e.data) as Record<string, unknown> }));
  assert.deepEqual(transferProof(records), { fills: 5, proven: 1, unproven: 4 });
  assert.deepEqual(transferProof([records[0]!, { event: "FillSettled", data: {} }]), { fills: 1, proven: 0, unproven: 1 });
});

test("EscrowEvent carries its amount unit (#89)", () => {
  const disc = idl.events.find((e) => e.name === "EscrowEvent")!.discriminator;
  const ev = { action: "AssetShortfall", session: S, escrow: S, owner: S, amount: 7n, unit: "AssetBase", caller: S };
  const line = "Program data: " + Buffer.from([...disc, ...encode({ defined: { name: "EscrowEvent" } }, ev)]).toString("base64");
  const [got] = eventsFromLogs([`Program ${PROGRAM_ID} invoke [1]`, line, `Program ${PROGRAM_ID} success`]);
  assert.deepEqual(got, { name: "EscrowEvent", data: ev });
});
