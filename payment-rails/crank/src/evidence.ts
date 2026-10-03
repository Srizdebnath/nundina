/**
 * Evidence export for cash events (R4): every rails event, decoded from
 * transaction logs, written as JSON Lines plus a manifest with a SHA-256
 * over the file, and the manifest **signed** (ed25519) by the exporting
 * key (#127, SRS 08 §6 "signed JSON files") — so an auditor can re-derive
 * every cash movement, prove the export wasn't edited, and attribute it.
 * `verifyEvidence` checks all three without a key or an RPC.
 *
 * Only `Program data:` lines emitted while the rails program itself is the
 * executing frame are attributed to it (the invoke stack is tracked from
 * the logs), so a CPI'd program can't inject look-alike events.
 */
import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import {
  getBase58Decoder,
  getBase58Encoder,
  getBase64Encoder,
  getPublicKeyFromAddress,
  isAddress,
  signBytes,
  verifySignature,
  type Address,
  type KeyPairSigner,
  type Signature,
  type SignatureBytes,
} from "@solana/kit";
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
 * AND names an exemption basis AND lists the lots it drew from. Since
 * #133 every fill runs credit-gate's settle-time check and records all
 * three; fills settled before it carry `NotRecorded` / `NotPerformed` and
 * stay `unproven`, so a bare event count never reads as compliance
 * evidence.
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

/**
 * Who vouches for a bundle (#127): an ed25519 signature by `signer` over
 * `EVIDENCE_DOMAIN` + the canonical JSON of the manifest without this field
 * (keys sorted at every level, so key order on disk doesn't matter). The
 * manifest carries the SHA-256 of `events.jsonl`, so the signature covers
 * every event.
 */
export interface Attestation {
  scheme: "ed25519";
  signer: Address;
  /** base58, like a transaction signature. */
  signature: string;
}

/** Domain separation: a bundle signature can't be replayed as a transaction or message. */
export const EVIDENCE_DOMAIN = "nundina:payment-rails:evidence-manifest:v1\n";

/** JSON with object keys sorted at every level. Pure. */
export function canonicalJson(v: unknown): string {
  if (Array.isArray(v)) return `[${v.map(canonicalJson).join(",")}]`;
  if (v && typeof v === "object") {
    const o = v as Record<string, unknown>;
    return `{${Object.keys(o)
      .filter((k) => o[k] !== undefined)
      .sort()
      .map((k) => `${JSON.stringify(k)}:${canonicalJson(o[k])}`)
      .join(",")}}`;
  }
  return JSON.stringify(v);
}

/** The exact bytes a manifest's attestation signs. Pure. */
export function signedPayload(manifest: UnsignedManifest): Uint8Array {
  const { attestation: _, ...unsigned } = manifest as Manifest;
  return new TextEncoder().encode(EVIDENCE_DOMAIN + canonicalJson(unsigned));
}

/** Sign a manifest with `signer` (the exporting crank's key). */
export async function signManifest(manifest: UnsignedManifest, signer: KeyPairSigner): Promise<Manifest> {
  const sig = await signBytes(signer.keyPair.privateKey, signedPayload(manifest));
  return {
    ...manifest,
    attestation: { scheme: "ed25519", signer: signer.address, signature: getBase58Decoder().decode(sig) },
  };
}

export type UnsignedManifest = Omit<Manifest, "attestation">;

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
  attestation: Attestation;
}

/**
 * Walk the program's signature history (newest first, paged), decode every
 * successful transaction's events, and write `<outDir>/events.jsonl` (oldest
 * first) + `<outDir>/manifest.json`.
 */
export async function exportEvidence(
  rpc: R,
  outDir: string,
  signer: KeyPairSigner,
  opts: { limit?: number; address?: Address } = {},
): Promise<Manifest> {
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
  const unsigned: UnsignedManifest = {
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
  const manifest = await signManifest(unsigned, signer);
  writeFileSync(join(outDir, "manifest.json"), JSON.stringify(manifest, null, 2) + "\n");
  return manifest;
}

export interface EvidenceCheck {
  ok: boolean;
  signer: string | null;
  problems: string[];
}

/**
 * Keyless, offline check of an exported bundle (#127): `events.jsonl`
 * hashes to the manifest's `sha256`, its event count matches, and the
 * manifest's attestation verifies, optionally against an expected signer
 * (the crank key the auditor was told to trust).
 */
export async function verifyEvidence(dir: string, expectedSigner?: string): Promise<EvidenceCheck> {
  const problems: string[] = [];
  const manifest = JSON.parse(readFileSync(join(dir, "manifest.json"), "utf8")) as Manifest;
  const body = readFileSync(join(dir, manifest.file), "utf8");
  if (createHash("sha256").update(body).digest("hex") !== manifest.sha256) {
    problems.push(`${manifest.file} does not hash to the manifest's sha256 (edited after export)`);
  }
  const lines = body.split("\n").filter((l) => l.length > 0).length;
  if (lines !== manifest.events) problems.push(`${manifest.file} has ${lines} events, manifest says ${manifest.events}`);

  const a = manifest.attestation;
  if (!a || a.scheme !== "ed25519" || !isAddress(a.signer) || typeof a.signature !== "string") {
    problems.push("manifest is unsigned or its attestation is malformed");
    return { ok: false, signer: null, problems };
  }
  if (expectedSigner !== undefined && a.signer !== expectedSigner) {
    problems.push(`signed by ${a.signer}, expected ${expectedSigner}`);
  }
  let valid = false;
  try {
    const sig = getBase58Encoder().encode(a.signature) as SignatureBytes;
    valid = sig.length === 64 && (await verifySignature(await getPublicKeyFromAddress(a.signer), sig, signedPayload(manifest)));
  } catch {
    valid = false;
  }
  if (!valid) problems.push("attestation signature does not verify over the manifest");
  return { ok: problems.length === 0, signer: a.signer, problems };
}
