/**
 * End to end on the local demo cluster (`scripts/localnet.sh`): the session
 * page's own transaction builders against the real programs.
 *
 *   1. a new bidder (b4) commits a sealed bid with `commitBidIx` (lib/tx.ts);
 *      the session now holds its commitment, byte for byte;
 *   2. once the commit window closes, every order — the four sealed in the
 *      fixture and b4's — is revealed with `revealIx`;
 *   3. once reveals close, the payment-rails crank's phase runner (#149)
 *      drives clear → allocate → settle each fill → finish;
 *   4. the session page's loader reads it back: Printed, clearing price,
 *      every fill settled DvP, the print recorded in mark-engine.
 *
 *   npm run localnet:e2e      (validator from `COMMIT_SECS=90 REVEAL_SECS=45 scripts/localnet.sh`)
 */
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import {
  AccountRole,
  address,
  appendTransactionMessageInstructions,
  createKeyPairSignerFromBytes,
  createTransactionMessage,
  generateKeyPairSigner,
  getAddressEncoder,
  getBase64EncodedWireTransaction,
  lamports,
  pipe,
  setTransactionMessageFeePayerSigner,
  setTransactionMessageLifetimeUsingBlockhash,
  signTransactionMessageWithSigners,
  type Address,
  type Instruction,
  type KeyPairSigner,
} from "@solana/kit";
import { concat, fromHex, hex, u64le } from "../lib/borsh";
import { SYSTEM_PROGRAM, TOKEN_PROGRAM } from "../lib/programs";
import { chainNow, getAccount, rpc } from "../lib/rpc";
import { loadSession } from "../lib/session";
import { commitBidIx, commitment, revealIx } from "../lib/tx";

const OUT = process.env.OUT ?? join(import.meta.dirname, "..", ".localnet");
const fx = JSON.parse(readFileSync(join(OUT, "fixture.json"), "utf8"));
const key = async (name: string) =>
  createKeyPairSignerFromBytes(Uint8Array.from(JSON.parse(readFileSync(join(OUT, "keys", `${name}.json`), "utf8"))));

function assert(c: unknown, m: string): asserts c {
  if (!c) throw new Error(`FAIL: ${m}`);
  console.log(`ok   ${m}`);
}

async function send(ixs: Instruction[], payer: KeyPairSigner): Promise<string> {
  const { value: bh } = await rpc.getLatestBlockhash({ commitment: "confirmed" }).send();
  const msg = pipe(
    createTransactionMessage({ version: 0 }),
    (m) => setTransactionMessageFeePayerSigner(payer, m),
    (m) => setTransactionMessageLifetimeUsingBlockhash(bh, m),
    (m) => appendTransactionMessageInstructions(ixs, m),
  );
  const tx = await signTransactionMessageWithSigners(msg);
  const sig = await rpc.sendTransaction(getBase64EncodedWireTransaction(tx), { encoding: "base64", preflightCommitment: "confirmed" }).send();
  for (let i = 0; i < 60; i++) {
    const { value } = await rpc.getSignatureStatuses([sig]).send();
    if (value[0]?.err) throw new Error(`${sig}: ${JSON.stringify(value[0].err, (_k, v) => (typeof v === "bigint" ? `${v}` : v))}`);
    if (value[0]?.confirmationStatus === "confirmed" || value[0]?.confirmationStatus === "finalized") return sig;
    await new Promise((r) => setTimeout(r, 500));
  }
  throw new Error(`unconfirmed ${sig}`);
}

async function waitUntil(t: number, what: string) {
  for (;;) {
    const now = await chainNow();
    if (now >= t) return;
    console.log(`…    waiting ${t - now}s for ${what}`);
    await new Promise((r) => setTimeout(r, Math.min(10, t - now) * 1000));
  }
}

const enc = getAddressEncoder();
/** A fresh SPL token account for `owner`, funded by the fixture's mint authority. */
async function fundedUsdc(owner: KeyPairSigner, authority: KeyPairSigner, mint: Address, amount: bigint): Promise<Address> {
  const acct = await generateKeyPairSigner();
  const rent = await rpc.getMinimumBalanceForRentExemption(165n).send();
  const create: Instruction = {
    programAddress: SYSTEM_PROGRAM,
    accounts: [
      { address: owner.address, role: AccountRole.WRITABLE_SIGNER, signer: owner },
      { address: acct.address, role: AccountRole.WRITABLE_SIGNER, signer: acct },
    ],
    data: concat(Uint8Array.of(0, 0, 0, 0), u64le(rent), u64le(165n), enc.encode(TOKEN_PROGRAM) as Uint8Array),
  } as Instruction;
  const init: Instruction = {
    programAddress: TOKEN_PROGRAM,
    accounts: [
      { address: acct.address, role: AccountRole.WRITABLE },
      { address: mint, role: AccountRole.READONLY },
    ],
    data: concat(Uint8Array.of(18), enc.encode(owner.address) as Uint8Array),
  };
  const mintTo: Instruction = {
    programAddress: TOKEN_PROGRAM,
    accounts: [
      { address: mint, role: AccountRole.WRITABLE },
      { address: acct.address, role: AccountRole.WRITABLE },
      { address: authority.address, role: AccountRole.READONLY_SIGNER, signer: authority },
    ],
    data: concat(Uint8Array.of(7), u64le(amount)),
  } as Instruction;
  await send([create, init, mintTo], owner);
  return acct.address;
}

const session = address(fx.sessions.open);
const before = await loadSession(session);
assert(before && before.phase === "Commit" && before.orders.length === 4, "open session has 4 sealed orders, phase Commit");
const rails = before!.railsSession;

// 1. b4 commits through the page's builder.
const b4 = await generateKeyPairSigner();
await rpc.requestAirdrop(b4.address, lamports(2_000_000_000n)).send();
for (let i = 0; i < 30 && !(await getAccount(b4.address)); i++) await new Promise((r) => setTimeout(r, 500));
const authority = await key("authority");
const usdc = await fundedUsdc(b4, authority, address(fx.usdcMint), 50_000_000n);
const b4Order = { side: "Buy" as const, limit: 900_000n, qty: 1_000_000n, nonce: crypto.getRandomValues(new Uint8Array(32)) };
const c = await commitment(b4Order.side, b4Order.limit, b4Order.qty, b4Order.nonce, b4.address);
const commitSig = await send(
  [
    await commitBidIx(
      { session, railsSession: rails, owner: b4.address, usdcMint: address(fx.usdcMint), ownerUsdc: usdc, usdcTokenProgram: TOKEN_PROGRAM },
      c,
      1_000_000n,
    ),
  ],
  b4,
);
const afterCommit = await loadSession(session);
const mine = afterCommit!.orders.find((o) => o.owner === b4.address);
assert(mine && hex(mine.commitment) === hex(c), `commit_bid landed (${commitSig.slice(0, 16)}…) with the page's commitment hash`);
assert(mine!.escrowed === 1_000_000n, "1 USDC escrowed in payment-rails for b4");

// 2. Reveal every order.
await waitUntil(fx.commitEnd, "commit window to close");
const preimages: { who: KeyPairSigner; side: "Buy" | "Sell"; limit: bigint; qty: bigint; nonce: Uint8Array }[] = [];
const byOwner: Record<string, string> = Object.fromEntries(Object.entries(fx.wallets as Record<string, string>).map(([k, v]) => [v, k]));
for (const o of fx.openOrders as { owner: string; side: "Buy" | "Sell"; limit: number; qty: number; nonce: string }[]) {
  preimages.push({ who: await key(byOwner[o.owner]!), side: o.side, limit: BigInt(o.limit), qty: BigInt(o.qty), nonce: fromHex(o.nonce) });
}
preimages.push({ who: b4, ...b4Order });
for (const p of preimages) {
  await send([await revealIx(session, rails, p.who.address, p.limit, p.qty, p.nonce)], p.who);
}
const revealed = await loadSession(session);
assert(revealed!.orders.every((o) => o.revealed), "all 5 orders revealed through revealIx");

// 3. The phase crank (#149) runs the session to its print.
await waitUntil(fx.revealEnd, "reveal window to close");
const crankDir = join(import.meta.dirname, "..", "..", "payment-rails", "crank");
const log = execFileSync("npx", ["tsx", "src/cli.ts", "phases", "--session", session], {
  cwd: crankDir,
  env: { ...process.env, RPC_URL: "http://127.0.0.1:8899", KEYPAIR: join(OUT, "keys", "crank.json") },
  encoding: "utf8",
});
console.log(log.trim().split("\n").map((l) => `     ${l}`).join("\n"));

// 4. What the session page shows.
const done = await loadSession(session);
assert(done!.phase === "Printed", `phase Printed (was ${done!.phase})`);
// Book: ask 8 @ 0.92; bids 5 @ 0.96, 4 @ 0.945, 3 @ 0.925, b4 1 @ 0.90.
// Max volume 8 with least imbalance at 0.945 (demand 9 vs supply 8).
assert(done!.clearPrice === 945_000n, `uniform clearing price 0.945 (got ${done!.clearPrice})`);
assert(done!.clearedVolume === 8_000_000n, "8 units cleared");
assert(done!.fills.length > 0 && done!.fills.every((f) => f.settled), `all ${done!.fills.length} fills settled DvP`);
assert(done!.orders.find((o) => o.owner === b4.address)!.alloc === 0n, "b4's 0.90 bid is out of the money");
assert(done!.print !== null && done!.print.price === 945_000n, "print recorded in mark-engine at the clearing price");
console.log("localnet e2e: PASS");
