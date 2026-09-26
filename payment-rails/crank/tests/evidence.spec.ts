import assert from "node:assert/strict";
import { test } from "node:test";
import { address } from "@solana/kit";
import { encode, idl, PROGRAM_ID } from "../src/idl.ts";
import { eventsFromLogs, toJson } from "../src/evidence.ts";

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
