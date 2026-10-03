// #127: exported evidence is signed, and the offline verifier rejects any
// edit to the events, any edit to the manifest, a different signer, and an
// unsigned bundle.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { generateKeyPairSigner } from "@solana/kit";
import {
  canonicalJson,
  signManifest,
  signedPayload,
  verifyEvidence,
  type UnsignedManifest,
} from "../src/evidence.ts";
import { PROGRAM_ID } from "../src/idl.ts";

const EVENTS =
  [
    { signature: "s1", slot: "10", blockTime: null, event: "FillSettled", data: { qty: "5", gross: "7" } },
    { signature: "s2", slot: "11", blockTime: "1700000000", event: "EscrowEvent", data: { action: "Refunded" } },
  ]
    .map((r) => JSON.stringify(r))
    .join("\n") + "\n";

function unsigned(body: string): UnsignedManifest {
  return {
    program: PROGRAM_ID,
    generatedAt: "2026-10-03T00:00:00.000Z",
    transactions: 2,
    events: 2,
    byEvent: { FillSettled: 1, EscrowEvent: 1 },
    transferProof: { fills: 0, proven: 0, unproven: 0 },
    slotRange: ["10", "11"],
    file: "events.jsonl",
    sha256: createHash("sha256").update(body).digest("hex"),
  };
}

async function bundle(edit?: (dir: string) => void) {
  const signer = await generateKeyPairSigner();
  const dir = mkdtempSync(join(tmpdir(), "evidence-"));
  writeFileSync(join(dir, "events.jsonl"), EVENTS);
  const m = await signManifest(unsigned(EVENTS), signer);
  writeFileSync(join(dir, "manifest.json"), JSON.stringify(m, null, 2) + "\n");
  edit?.(dir);
  return { dir, signer };
}

const patchManifest = (dir: string, f: (m: Record<string, unknown>) => void) => {
  const p = join(dir, "manifest.json");
  const m = JSON.parse(readFileSync(p, "utf8"));
  f(m);
  writeFileSync(p, JSON.stringify(m));
};

test("canonicalJson sorts keys at every level, so key order on disk is irrelevant", () => {
  assert.equal(canonicalJson({ b: 1, a: { d: [2, { z: 1, y: 2 }], c: null } }), '{"a":{"c":null,"d":[2,{"y":2,"z":1}]},"b":1}');
  assert.equal(canonicalJson({ a: 1, b: 2 }), canonicalJson({ b: 2, a: 1 }));
});

test("the signed payload is domain-separated and excludes the attestation", async () => {
  const signer = await generateKeyPairSigner();
  const m = await signManifest(unsigned(EVENTS), signer);
  const text = new TextDecoder().decode(signedPayload(m));
  assert.ok(text.startsWith("nundina:payment-rails:evidence-manifest:v1\n"));
  assert.ok(!text.includes("attestation"));
  assert.deepEqual(signedPayload(m), signedPayload(unsigned(EVENTS)));
});

test("a signed bundle verifies, with and without an expected signer", async () => {
  const { dir, signer } = await bundle();
  const r = await verifyEvidence(dir, signer.address);
  assert.deepEqual(r, { ok: true, signer: signer.address, problems: [] });
  assert.equal((await verifyEvidence(dir)).ok, true);
});

test("reordered manifest keys still verify (canonical form)", async () => {
  const { dir } = await bundle((d) =>
    patchManifest(d, (m) => {
      const entries = Object.entries(m).reverse();
      for (const k of Object.keys(m)) delete m[k];
      Object.assign(m, Object.fromEntries(entries));
    }),
  );
  assert.equal((await verifyEvidence(dir)).ok, true);
});

test("an edited event is rejected (hash)", async () => {
  const { dir } = await bundle((d) => writeFileSync(join(d, "events.jsonl"), EVENTS.replace('"qty":"5"', '"qty":"6"')));
  const r = await verifyEvidence(dir);
  assert.equal(r.ok, false);
  assert.match(r.problems.join(), /does not hash/);
});

test("an edited event re-hashed into the manifest is rejected (signature)", async () => {
  const edited = EVENTS.replace('"gross":"7"', '"gross":"8"');
  const { dir } = await bundle((d) => {
    writeFileSync(join(d, "events.jsonl"), edited);
    patchManifest(d, (m) => (m.sha256 = createHash("sha256").update(edited).digest("hex")));
  });
  const r = await verifyEvidence(dir);
  assert.equal(r.ok, false);
  assert.deepEqual(r.problems, ["attestation signature does not verify over the manifest"]);
});

test("a dropped event is rejected", async () => {
  const one = EVENTS.split("\n")[0] + "\n";
  const { dir } = await bundle((d) => writeFileSync(join(d, "events.jsonl"), one));
  const r = await verifyEvidence(dir);
  assert.equal(r.ok, false);
  assert.ok(r.problems.some((p) => /has 1 events, manifest says 2/.test(p)));
});

test("an edited manifest field is rejected", async () => {
  const { dir } = await bundle((d) => patchManifest(d, (m) => ((m.transferProof as { proven: number }).proven = 1)));
  assert.deepEqual((await verifyEvidence(dir)).problems, ["attestation signature does not verify over the manifest"]);
});

test("a different expected signer is rejected", async () => {
  const { dir } = await bundle();
  const other = await generateKeyPairSigner();
  const r = await verifyEvidence(dir, other.address);
  assert.equal(r.ok, false);
  assert.match(r.problems.join(), /expected/);
});

test("a re-signed bundle names its real signer, so swapping keys is visible", async () => {
  const mallory = await generateKeyPairSigner();
  const { dir, signer } = await bundle(async () => {});
  const m = JSON.parse(readFileSync(join(dir, "manifest.json"), "utf8"));
  const resigned = await signManifest(m, mallory);
  writeFileSync(join(dir, "manifest.json"), JSON.stringify(resigned));
  assert.equal((await verifyEvidence(dir)).signer, mallory.address);
  assert.equal((await verifyEvidence(dir, signer.address)).ok, false);
});

test("an unsigned bundle (the pre-#127 format) is rejected", async () => {
  const { dir } = await bundle((d) => patchManifest(d, (m) => delete m.attestation));
  assert.deepEqual((await verifyEvidence(dir)).problems, ["manifest is unsigned or its attestation is malformed"]);
});

test("a garbage signature is rejected, not thrown", async () => {
  const { dir } = await bundle((d) => patchManifest(d, (m) => ((m.attestation as { signature: string }).signature = "0OIl")));
  assert.equal((await verifyEvidence(dir)).ok, false);
});
