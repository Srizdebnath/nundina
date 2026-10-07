/**
 * nav-publisher (SRS 08 §6, §3.2): read the treasury and the supply at one
 * slot, compute the NAV with `credit-math`'s arithmetic (`nav.ts`), hash the
 * raw inputs, and send `mark-engine::publish_nav` signed by a key in the
 * feed's publisher set. Writes the inputs document next to the signature so
 * anyone can recompute the published number.
 *
 * Treasury value comes from one of:
 *   - `treasury:<npcs-treasury PDA>` — the I1 treasury's USDC vault (#148);
 *   - `sleeve:<USDC token account>` — the backup / manual sleeve (devnet
 *     today: the 1 USDC deposit behind the 1 NPCS minted to holder H).
 *
 * Not done here: §3.2 step 5 (a SAS attestation of the NAV) — mark-engine's
 * `publish_nav` takes no attestation account, so there is nothing on chain
 * to verify it against; the publisher-set signature is the control.
 *
 * Run weekly (cron / systemd timer); `--dry-run` computes without sending.
 */
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import {
  AccountRole,
  address,
  appendTransactionMessageInstructions,
  createTransactionMessage,
  getAddressDecoder,
  getAddressEncoder,
  getBase64EncodedWireTransaction,
  getProgramDerivedAddress,
  pipe,
  setTransactionMessageFeePayerSigner,
  setTransactionMessageLifetimeUsingBlockhash,
  signTransactionMessageWithSigners,
  type Address,
  type Instruction,
  type KeyPairSigner,
} from "@solana/kit";
import { concat, fromBase64, hex, u64le, type Struct } from "./codec.ts";
import { canonical, inputsHash, MODEL_VERSION, weeklyPublish, type NavInputs } from "./nav.ts";
import { PROGRAMS, programId } from "./programs.ts";
import type { R } from "./rpc.ts";

const CLOCK = address("SysvarC1ock11111111111111111111111111111111");
const MARK = programId("mark-engine");

export async function feedPda(mint: Address): Promise<Address> {
  const [a] = await getProgramDerivedAddress({
    programAddress: MARK,
    seeds: [new TextEncoder().encode("feed"), getAddressEncoder().encode(mint)],
  });
  return a;
}

/** Base mint layout (SPL and Token-2022): supply u64 @36. */
export const mintSupply = (data: Uint8Array) => new DataView(data.buffer, data.byteOffset).getBigUint64(36, true);
/** Token account layout: mint @0, owner @32, amount u64 @64. */
export const tokenAmount = (data: Uint8Array) => new DataView(data.buffer, data.byteOffset).getBigUint64(64, true);
const tokenMint = (data: Uint8Array) => getAddressDecoder().decode(data.subarray(0, 32));

export interface Computed {
  feed: Address;
  /** `null` only in a dry run against a cluster with no feed yet. */
  feedState: Struct | null;
  inputs: NavInputs;
  hash: Uint8Array;
  gross: bigint;
  fee: bigint;
  published: bigint;
}

/** One consistent snapshot (a single `getMultipleAccounts`), then the arithmetic. */
export async function computeNav(
  rpc: R,
  o: { mint: Address; source: string; feeBps: bigint; allowNoFeed?: boolean },
): Promise<Computed> {
  const [kind, acct] = o.source.split(":") as [string, string];
  if ((kind !== "sleeve" && kind !== "treasury") || !acct) throw new Error("--source must be sleeve:<usdc account> or treasury:<npcs-treasury PDA>");
  const feed = await feedPda(o.mint);
  let sleeve = address(acct);
  if (kind === "treasury") {
    const t = await rpc.getAccountInfo(address(acct), { encoding: "base64" }).send();
    if (!t.value) throw new Error(`treasury ${acct} not found`);
    const tr = PROGRAMS["npcs-treasury"].decodeAccount("Treasury", fromBase64(t.value.data[0]));
    if (tr.asset_mint !== o.mint) throw new Error(`treasury ${acct} is for ${tr.asset_mint}, not ${o.mint}`);
    sleeve = tr.vault as Address;
  }
  const { context, value } = await rpc
    .getMultipleAccounts([o.mint, sleeve, feed, CLOCK], { encoding: "base64", commitment: "confirmed" })
    .send();
  const [mintA, sleeveA, feedA, clockA] = value;
  if (!mintA) throw new Error(`mint ${o.mint} not found`);
  if (!sleeveA) throw new Error(`treasury account ${sleeve} not found`);
  if (!feedA && !o.allowNoFeed) throw new Error(`no mark-engine feed for ${o.mint} (${feed})`);
  const sleeveData = fromBase64(sleeveA.data[0]);
  const feedState = feedA ? PROGRAMS["mark-engine"].decodeAccount("Feed", fromBase64(feedA.data[0])) : null;
  const now = new DataView(fromBase64(clockA!.data[0]).buffer).getBigInt64(32, true);
  const supply = mintSupply(fromBase64(mintA.data[0]));
  const treasury = tokenAmount(sleeveData);
  const prev = feedState && (feedState.publications as bigint) > 0n ? (feedState.computed_ts as bigint) : now;
  const delta = now > prev ? now - prev : 0n;
  if (kind === "sleeve" && tokenMint(sleeveData) === o.mint) throw new Error("the sleeve must be a USDC account, not an NPCS account");
  const inputs: NavInputs = {
    model_version: MODEL_VERSION,
    asset_mint: o.mint,
    supply_base: supply.toString(),
    treasury_source: kind,
    treasury_account: sleeve,
    treasury_micros: treasury.toString(),
    slot: context.slot.toString(),
    fee_bps: o.feeBps.toString(),
    delta_seconds: delta.toString(),
    previous_computed_ts: prev.toString(),
    computed_ts: now.toString(),
  };
  const r = weeklyPublish(treasury, supply, o.feeBps, delta);
  return { feed, feedState, inputs, hash: inputsHash(inputs), ...r };
}

export function u128le(n: bigint): Uint8Array {
  const b = new Uint8Array(16);
  const v = new DataView(b.buffer);
  v.setBigUint64(0, n & 0xffff_ffff_ffff_ffffn, true);
  v.setBigUint64(8, n >> 64n, true);
  return b;
}

export function publishNavIx(feed: Address, publisher: Address, c: Pick<Computed, "published" | "inputs" | "hash">): Instruction {
  const disc = PROGRAMS["mark-engine"].idl.instructions.find((i) => i.name === "publish_nav")!.discriminator;
  const ts = new Uint8Array(8);
  new DataView(ts.buffer).setBigInt64(0, BigInt(c.inputs.computed_ts), true);
  const mv = new Uint8Array(4);
  new DataView(mv.buffer).setUint32(0, c.inputs.model_version, true);
  return {
    programAddress: MARK,
    accounts: [
      { address: feed, role: AccountRole.WRITABLE },
      { address: publisher, role: AccountRole.READONLY_SIGNER },
    ],
    data: concat(Uint8Array.from(disc), u128le(c.published), ts, mv, c.hash),
  };
}

export async function sendAndPoll(rpc: R, signer: KeyPairSigner, ixs: Instruction[]): Promise<string> {
  const { value: bh } = await rpc.getLatestBlockhash({ commitment: "confirmed" }).send();
  const msg = pipe(
    createTransactionMessage({ version: 0 }),
    (m) => setTransactionMessageFeePayerSigner(signer, m),
    (m) => setTransactionMessageLifetimeUsingBlockhash(bh, m),
    (m) => appendTransactionMessageInstructions(ixs, m),
  );
  const tx = await signTransactionMessageWithSigners(msg);
  const sig = await rpc.sendTransaction(getBase64EncodedWireTransaction(tx), { encoding: "base64", preflightCommitment: "confirmed" }).send();
  for (let i = 0; i < 90; i++) {
    const { value } = await rpc.getSignatureStatuses([sig]).send();
    const st = value[0];
    if (st?.err) throw new Error(`${sig} failed: ${JSON.stringify(st.err, (_k, v) => (typeof v === "bigint" ? v.toString() : v))}`);
    if (st?.confirmationStatus === "confirmed" || st?.confirmationStatus === "finalized") return sig;
    await new Promise((r) => setTimeout(r, 1000));
  }
  throw new Error(`not confirmed: ${sig}`);
}

export async function publish(
  rpc: R,
  o: { mint: Address; source: string; feeBps: bigint; publisher: KeyPairSigner | null; out?: string },
): Promise<{ computed: Computed; signature: string | null; evidence: string | null }> {
  const c = await computeNav(rpc, { ...o, allowNoFeed: !o.publisher });
  let signature: string | null = null;
  if (o.publisher) {
    const publishers = (c.feedState!.publishers as string[]).slice(0, Number(c.feedState!.n_publishers));
    if (!publishers.includes(o.publisher.address)) throw new Error(`${o.publisher.address} is not in the feed's publisher set`);
    signature = await sendAndPoll(rpc, o.publisher, [publishNavIx(c.feed, o.publisher.address, c)]);
  }
  let evidence: string | null = null;
  if (o.out) {
    mkdirSync(o.out, { recursive: true });
    evidence = join(o.out, `nav-${c.inputs.asset_mint}-${c.inputs.computed_ts}.json`);
    writeFileSync(
      evidence,
      JSON.stringify(
        {
          feed: c.feed,
          inputs: c.inputs,
          canonical_inputs: canonical(c.inputs),
          inputs_hash: hex(c.hash),
          nav_gross_pico: c.gross.toString(),
          fee_pico: c.fee.toString(),
          nav_published_pico: c.published.toString(),
          signature,
        },
        null,
        2,
      ) + "\n",
    );
  }
  return { computed: c, signature, evidence };
}

export { u64le };
