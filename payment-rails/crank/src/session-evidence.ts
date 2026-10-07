/**
 * Session evidence bundle (SRS 08 §6 evidence-exporter, AC-12, #141): one
 * auction session, everything an auditor needs to re-derive it.
 *
 *   events.jsonl  — every event of auction-engine, payment-rails,
 *                   credit-gate and mark-engine in every successful
 *                   transaction that touched the session or its rails
 *                   CashSession, oldest first, attributed by invoke stack;
 *   snapshot.json — the state the events produced, read at export: the
 *                   session (params, phase, clearing, orders, fills), the
 *                   rails CashSession, the mark feed (NAV, model_version,
 *                   inputs_hash, state) and the print record;
 *   manifest.json — counts, AE-P8 transfer proof, SHA-256 of both files,
 *                   ed25519-signed by the exporting key.
 *
 * `crank verify-export --dir <out>` checks both hashes and the signature
 * offline, like the program-wide bundle.
 */
import { createHash } from "node:crypto";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { getBase64Encoder, getProgramDerivedAddress, getAddressEncoder, type Address, type KeyPairSigner, type Signature } from "@solana/kit";
import { rawAccount, type R } from "./chain.ts";
import { signManifest, toJson, transferProof, type EvidenceRecord, type Manifest, type UnsignedManifest } from "./evidence.ts";
import { decodeAccount as railsAccount, loadIdl, makeCodec, PROGRAM_ID as RAILS, type Value } from "./idl.ts";
import { auction, AUCTION, MARK_ENGINE, type AuctionSession } from "./phases.ts";

const gate = makeCodec(loadIdl("credit_gate"));
const mark = makeCodec(loadIdl("mark_engine"));
const CODECS = new Map<string, { name: string; codec: ReturnType<typeof makeCodec> }>([
  [AUCTION, { name: "auction-engine", codec: auction }],
  [RAILS, { name: "payment-rails", codec: makeCodec(loadIdl("payment_rails")) }],
  [gate.programId, { name: "credit-gate", codec: gate }],
  [mark.programId, { name: "mark-engine", codec: mark }],
]);
const b64 = getBase64Encoder();

export interface SessionRecord extends EvidenceRecord {
  program: string;
}

/** Events of the four session programs in one transaction's logs, attributed by invoke stack. Pure. */
export function sessionEventsFromLogs(logs: readonly string[]): { program: string; name: string; data: Record<string, Value> }[] {
  const stack: string[] = [];
  const out: { program: string; name: string; data: Record<string, Value> }[] = [];
  for (const line of logs) {
    const invoke = line.match(/^Program (\w+) invoke \[\d+\]$/);
    if (invoke) { stack.push(invoke[1]!); continue; }
    if (/^Program \w+ (success|failed)/.test(line)) { stack.pop(); continue; }
    if (!line.startsWith("Program data: ")) continue;
    const p = CODECS.get(stack[stack.length - 1] ?? "");
    if (!p) continue;
    const ev = p.codec.decodeEvent(new Uint8Array(b64.encode(line.slice("Program data: ".length))));
    if (ev) out.push({ program: p.name, name: ev.name, data: ev.data });
  }
  return out;
}

async function signaturesOf(rpc: R, a: Address, limit: number) {
  const out: { signature: Signature; slot: bigint; err: unknown }[] = [];
  let before: Signature | undefined;
  while (out.length < limit) {
    const page = await rpc.getSignaturesForAddress(a, { limit: Math.min(1_000, limit - out.length), before, commitment: "confirmed" }).send();
    if (page.length === 0) break;
    out.push(...page.map((p) => ({ signature: p.signature, slot: p.slot, err: p.err })));
    before = page[page.length - 1]!.signature;
  }
  return out;
}

export async function exportSessionEvidence(
  rpc: R,
  outDir: string,
  signer: KeyPairSigner,
  session: Address,
  opts: { limit?: number } = {},
): Promise<Manifest> {
  const raw = await rawAccount(rpc, session);
  const s = raw && raw.owner === AUCTION ? auction.decodeAccount<AuctionSession>("Session", raw.data) : null;
  if (!s) throw new Error(`${session} is not an auction-engine session`);
  const limit = opts.limit ?? 5_000;
  const bySig = new Map<string, { signature: Signature; slot: bigint; err: unknown }>();
  for (const a of [session, s.rails_session]) for (const x of await signaturesOf(rpc, a, limit)) bySig.set(x.signature, x);
  const sigs = [...bySig.values()].sort((a, b) => (a.slot < b.slot ? -1 : a.slot > b.slot ? 1 : 0));

  const records: SessionRecord[] = [];
  let txs = 0;
  for (const x of sigs) {
    if (x.err) continue;
    const tx = await rpc.getTransaction(x.signature, { encoding: "json", maxSupportedTransactionVersion: 0, commitment: "confirmed" }).send();
    const logs = tx?.meta?.logMessages;
    if (!logs) continue;
    txs++;
    for (const ev of sessionEventsFromLogs(logs)) {
      records.push({
        signature: x.signature,
        slot: x.slot.toString(),
        blockTime: tx.blockTime === null ? null : tx.blockTime.toString(),
        program: ev.program,
        event: ev.name,
        data: toJson(ev.data) as Record<string, unknown>,
      });
    }
  }

  // State at export.
  const [printRecord] = await getProgramDerivedAddress({
    programAddress: MARK_ENGINE,
    seeds: [new TextEncoder().encode("print"), getAddressEncoder().encode(s.feed), getAddressEncoder().encode(session)],
  });
  const [railsRaw, feedRaw, printRaw] = await Promise.all([rawAccount(rpc, s.rails_session), rawAccount(rpc, s.feed), rawAccount(rpc, printRecord)]);
  const snapshot = {
    session: { address: session, ...(toJson(s) as object) },
    rails_session: railsRaw ? { address: s.rails_session, ...(toJson(railsAccount("CashSession", railsRaw.data)) as object) } : null,
    feed: feedRaw ? { address: s.feed, ...(toJson(mark.decodeAccount("Feed", feedRaw.data)) as object) } : null,
    print: printRaw ? { address: printRecord, ...(toJson(mark.decodeAccount("PrintRecord", printRaw.data)) as object) } : null,
  };

  mkdirSync(outDir, { recursive: true });
  const body = records.map((r) => JSON.stringify(r)).join("\n") + (records.length ? "\n" : "");
  writeFileSync(join(outDir, "events.jsonl"), body);
  const snap = JSON.stringify(snapshot, null, 2) + "\n";
  writeFileSync(join(outDir, "snapshot.json"), snap);
  const byEvent: Record<string, number> = {};
  for (const r of records) byEvent[`${r.program}:${r.event}`] = (byEvent[`${r.program}:${r.event}`] ?? 0) + 1;
  const unsigned: UnsignedManifest = {
    program: `auction-engine session ${session}`,
    generatedAt: new Date().toISOString(),
    transactions: txs,
    events: records.length,
    byEvent,
    transferProof: transferProof(records.filter((r) => r.program === "payment-rails")),
    slotRange: records.length ? [records[0]!.slot, records[records.length - 1]!.slot] : null,
    file: "events.jsonl",
    sha256: createHash("sha256").update(body).digest("hex"),
    snapshot: { file: "snapshot.json", sha256: createHash("sha256").update(snap).digest("hex") },
  };
  const manifest = await signManifest(unsigned, signer);
  writeFileSync(join(outDir, "manifest.json"), JSON.stringify(manifest, null, 2) + "\n");
  return manifest;
}
