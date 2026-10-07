/**
 * One-off: snapshot the raw transactions of a local demo run (app/scripts/
 * localnet.sh + `npm run localnet:e2e` + a crank-runner tick) as indexer
 * test input. Real program logs, not hand-written ones.
 *
 *   RPC_URL=http://127.0.0.1:8899 npx tsx test/capture-localnet.ts
 */
import { writeFileSync } from "node:fs";
import type { Signature } from "@solana/kit";
import { fetchTx, type RawTx } from "../src/indexer.ts";
import { BY_ID } from "../src/programs.ts";
import { makeRpc, RPC_URL } from "../src/rpc.ts";

const rpc = makeRpc(RPC_URL);
const seen = new Map<string, RawTx>();
for (const [id] of BY_ID) {
  const sigs = await rpc.getSignaturesForAddress(id, { limit: 1000, commitment: "confirmed" }).send();
  for (const s of sigs) {
    if (seen.has(s.signature)) continue;
    const tx = await fetchTx(rpc, s.signature as Signature);
    if (tx) seen.set(s.signature, tx);
  }
}
const txs = [...seen.values()].sort((a, b) => Number(BigInt(a.slot) - BigInt(b.slot)));
writeFileSync(
  new URL("./fixtures/localnet-txs.json", import.meta.url),
  JSON.stringify(txs.map((t) => ({ ...t, slot: Number(t.slot), blockTime: t.blockTime === null ? null : Number(t.blockTime) })), null, 1) + "\n",
);
console.log(`${txs.length} transactions`);
