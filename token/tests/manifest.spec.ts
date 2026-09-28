// Offline drift tests: params.toml is the declared single source of truth
// (doc 09 §3), so every other file that restates one of its facts must agree
// with it. No network, no keys.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const read = (p: string) => readFileSync(new URL(p, import.meta.url), "utf8");

type Scalar = string | number | boolean;

/** Just enough TOML for params.toml: [sections], `key = scalar`, comments. */
function parseParams(src: string): Record<string, Record<string, Scalar>> {
  const out: Record<string, Record<string, Scalar>> = {};
  let section = "";
  for (const raw of src.split("\n")) {
    const line = raw.replace(/\s+#.*$/, "").replace(/^#.*$/, "").trim();
    if (!line) continue;
    const sec = line.match(/^\[([^\]]+)\]$/);
    if (sec) {
      section = sec[1];
      out[section] ??= {};
      continue;
    }
    const kv = line.match(/^([A-Za-z0-9_]+)\s*=\s*(.+)$/);
    assert.ok(kv, `unparsed params.toml line: ${raw}`);
    const v = kv[2].trim();
    let val: Scalar;
    if (v.startsWith('"')) val = JSON.parse(v);
    else if (v === "true" || v === "false") val = v === "true";
    else val = Number(v.replaceAll("_", ""));
    out[section][kv[1]] = val;
  }
  return out;
}

const params = parseParams(read("../params.toml"));
const metadata = JSON.parse(read("../metadata.json"));

test("mint decimals are 6 (I1: supply_base = deposits × 10⁶)", () => {
  assert.equal(params.mint.decimals, 6);
});

test("NAV precision matches credit-math PICO", () => {
  const nav = read("../../programs/credit-math/src/nav.rs");
  const m = nav.match(/pub const PICO: u128 = ([0-9_]+);/);
  assert.ok(m, "PICO not found in credit-math/src/nav.rs");
  assert.equal(params.nav.precision_pico, Number(m[1].replaceAll("_", "")));
});

test("metadata.json restates params.toml consistently", () => {
  assert.equal(metadata.symbol, params.mint.symbol);
  assert.equal(metadata.name, params.mint.name);
  assert.equal(metadata.extensions.default_account_state, params.extensions.default_account_state);
  assert.equal(metadata.extensions.governor_cap, params.policy.holder_cap);
  assert.equal(params.policy.seasoning_seconds, 7 * 86_400);
  assert.match(metadata.extensions.seasoning, /^7d\b/);
});

test("verify.ts forbids every extension params.toml declares absent", () => {
  const verify = read("../scripts/verify.ts");
  const block = verify.match(/const forbidden = \[([\s\S]*?)\];/);
  assert.ok(block, "forbidden list not found in scripts/verify.ts");
  const forbidden = new Set([...block[1].matchAll(/"([A-Za-z]+)"/g)].map((m) => m[1]));
  const kindOf: Record<string, string> = {
    transfer_hook: "TransferHook",
    transfer_fee: "TransferFeeConfig",
    interest_bearing: "InterestBearingConfig",
    confidential_transfer: "ConfidentialTransferMint",
    non_transferable: "NonTransferable",
  };
  for (const [key, value] of Object.entries(params.extensions)) {
    if (value !== false) continue;
    const kind = kindOf[key];
    assert.ok(kind, `params.toml extension '${key}' has no verify.ts kind mapping`);
    assert.ok(forbidden.has(kind), `verify.ts does not forbid ${kind} (params.toml ${key} = false)`);
  }
});
