// One decoder, two packages: services/src/codec.ts is app/lib/borsh.ts verbatim.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { eventsFromLogs } from "../src/logs.ts";

test("codec.ts is the app's borsh.ts byte for byte", () => {
  const ours = readFileSync(new URL("../src/codec.ts", import.meta.url), "utf8");
  const app = readFileSync(new URL("../../app/lib/borsh.ts", import.meta.url), "utf8");
  assert.equal(ours, app, "out of sync: run `npm run sync-codec` (copies app/lib/borsh.ts to services/src/codec.ts)");
});

test("invoke stack: the same bytes decode as the program on top of the stack, or not at all", () => {
  const AUCTION = "Cw6wNpVJUaofUDZewG8sDKZu3nnHga4GaJJvKn7rSod7";
  const RAILS = "GdWmRivxe1k76apQB7EgJ4yBq8Fi3Lw25E8V3XosPocn";
  const TOKEN = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
  const txs = JSON.parse(readFileSync(new URL("./fixtures/localnet-txs.json", import.meta.url), "utf8")) as { logs: string[] }[];
  // A real payment-rails FillSettled line from the demo run.
  const line = txs
    .flatMap((t) => t.logs)
    .find((l) => l.startsWith("Program data: ") && eventsFromLogs([`Program ${RAILS} invoke [1]`, l]).some((e) => e.name === "FillSettled"))!;
  assert.ok(line);
  const under = (prog: string, extra: string[] = []) => eventsFromLogs([`Program ${prog} invoke [1]`, ...extra, line]);
  assert.equal(under(RAILS)[0]!.program, "payment-rails");
  // After a nested CPI returns, the caller is on top again.
  assert.equal(under(RAILS, [`Program ${TOKEN} invoke [2]`, `Program ${TOKEN} success`])[0]!.program, "payment-rails");
  // Inside a program we do not index: skipped.
  assert.deepEqual(under(RAILS, [`Program ${TOKEN} invoke [2]`]), []);
  // Under auction-engine the rails layout is not an auction event of that name with these fields.
  const asAuction = under(AUCTION);
  assert.ok(asAuction.every((e) => e.program === "auction-engine" && !("gross" in e.data)));
});
