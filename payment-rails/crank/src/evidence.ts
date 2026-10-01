/**
 * Evidence export for cash events (R4): every rails event, decoded from
 * transaction logs, written as JSON Lines plus a manifest with a SHA-256
 * over the file — so an auditor can re-derive every cash movement and prove
 * the export wasn't edited.
 *
 * Only `Program data:` lines emitted while the rails program itself is the
 * executing frame are attributed to it (the invoke stack is tracked from
 * the logs), so a CPI'd program can't inject look-alike events.
 */
import { createHash } from "node:crypto";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { getBase64Encoder, type Address, type Signature } from "@solana/kit";
import type { R } from "./chain.ts";
import { decodeEvent, PROGRAM_ID, type Value } from "./idl.ts";

const b64 = getBase64Encoder();

export interface EvidenceRecord {
  signature: string;
  slot: string;
  blockTime: string | null;
  event: string;
  data: Record<string, unknown>;
}

/** Events emitted by `program` in one transaction's logs, in order. Pure. */
export function eventsFromLogs(logs: readonly string[], program: string = PROGRAM_ID) {
  const stack: string[] = [];
  const out: { name: string; data: Record<string, Value> }[] = [];
  for (const line of logs) {
    const invoke = line.match(/^Program (\w+) invoke \[\d+\]$/);
    if (invoke) { stack.push(invoke[1]!); continue; }
    if (/^Program \w+ (success|failed)/.test(line)) { stack.pop(); continue; }
    if (line.startsWith("Program data: ") && stack[stack.length - 1] === program) {
      const ev = decodeEvent(new Uint8Array(b64.encode(line.slice("Program data: ".length))));
      if (ev) out.push(ev);
    }
  }
  return out;
}

/** JSON-safe: bigint → decimal string, bytes → hex. */
export function toJson(v: unknown): unknown {
  if (typeof v === "bigint") return v.toString();
  if (v instanceof Uint8Array) return Buffer.from(v).toString("hex");
  if (Array.isArray(v)) return v.map(toJson);
  if (v && typeof v === "object") return Object.fromEntries(Object.entries(v).map(([k, x]) => [k, toJson(x)]));
  return v;
}

/**
 * What the AE-P8 transfer records in a bundle can prove (#88). A fill is
 * `proven` only if its `TransferInstruction` says the gate re-check passed
 * AND names an exemption basis AND lists the lots it drew from. Until the
 * lot ledger and settle-time gate check exist (#53) every fill is
 * `unproven`, and the manifest says so instead of letting the bare event
 * count read as compliance evidence (doc 07 L4/L21 are not met yet).
 */
export interface TransferProof {
  fills: number;
  proven: number;
  unproven: number;
}

/** Pure: classify the bundle's `TransferInstruction` records. */
export function transferProof(records: readonly Pick<EvidenceRecord, "event" | "data">[]): TransferProof {
  const transfers = records.filter((r) => r.event === "TransferInstruction");
  const proven = transfers.filter(
    (r) =>
      r.data.gate_check === "Passed" &&
      r.data.exemption_path !== "NotRecorded" &&
      Array.isArray(r.data.lot_ids) &&
      r.data.lot_ids.length > 0,
  ).length;
  return { fills: transfers.length, proven, unproven: transfers.length - proven };
}

export interface Manifest {
  program: string;
  generatedAt: string;
  transactions: number;
  events: number;
  byEvent: Record<string, number>;
  /** AE-P8 lawfulness coverage of the fills in this bundle (#88). */
  transferProof: TransferProof;
  slotRange: [string, string] | null;
  file: string;
  sha256: string;
}

/**
 * Walk the program's signature history (newest first, paged), decode every
 * successful transaction's events, and write `<outDir>/events.jsonl` (oldest
 * first) + `<outDir>/manifest.json`.
 */
export async function exportEvidence(rpc: R, outDir: string, opts: { limit?: number; address?: Address } = {}): Promise<Manifest> {
  const target = opts.address ?? PROGRAM_ID;
  const limit = opts.limit ?? 1_000;
  const sigs: { signature: Signature; slot: bigint; err: unknown }[] = [];
  let before: Signature | undefined;
  while (sigs.length < limit) {
    const page = await rpc
      .getSignaturesForAddress(target, { limit: Math.min(1_000, limit - sigs.length), before, commitment: "confirmed" })
      .send();
    if (page.length === 0) break;
    sigs.push(...page.map((p) => ({ signature: p.signature, slot: p.slot, err: p.err })));
    before = page[page.length - 1]!.signature;
  }

  const records: EvidenceRecord[] = [];
  let txs = 0;
  for (const s of sigs.reverse()) {
    if (s.err) continue; // failed transactions moved no cash
    const tx = await rpc
      .getTransaction(s.signature, { encoding: "json", maxSupportedTransactionVersion: 0, commitment: "confirmed" })
      .send();
    const logs = tx?.meta?.logMessages;
    if (!logs) continue;
    txs++;
    for (const ev of eventsFromLogs(logs)) {
      records.push({
        signature: s.signature,
        slot: s.slot.toString(),
        blockTime: tx.blockTime === null ? null : tx.blockTime.toString(),
        event: ev.name,
        data: toJson(ev.data) as Record<string, unknown>,
      });
    }
  }

  mkdirSync(outDir, { recursive: true });
  const body = records.map((r) => JSON.stringify(r)).join("\n") + (records.length ? "\n" : "");
  const file = join(outDir, "events.jsonl");
  writeFileSync(file, body);
  const byEvent: Record<string, number> = {};
  for (const r of records) byEvent[r.event] = (byEvent[r.event] ?? 0) + 1;
  const manifest: Manifest = {
    program: PROGRAM_ID,
    generatedAt: new Date().toISOString(),
    transactions: txs,
    events: records.length,
    byEvent,
    transferProof: transferProof(records),
    slotRange: records.length ? [records[0]!.slot, records[records.length - 1]!.slot] : null,
    file: "events.jsonl",
    sha256: createHash("sha256").update(body).digest("hex"),
  };
  writeFileSync(join(outDir, "manifest.json"), JSON.stringify(manifest, null, 2) + "\n");
  return manifest;
}
