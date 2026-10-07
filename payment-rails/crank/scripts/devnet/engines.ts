/**
 * Devnet engine wiring (#136–#138): after `solana program deploy` of
 * mark-engine, auction-engine and queue-claim, create their singleton /
 * per-asset accounts over the live NPCS mint, signed by the rails
 * authority (9WH1, the same key as payment-rails' config authority):
 *
 *   1. mark-engine `initialize_feed` for NPCS + `set_publishers([PUBLISHER])`;
 *   2. auction-engine `initialize_venue` (no forced-sale opener);
 *   3. mark-engine `set_print_source` = the auction-engine print PDA;
 *   4. queue-claim `init_queue` for NPCS (attester = the rails authority).
 *
 * The three init instructions are first-caller-wins (whoever creates the
 * account becomes its authority), so run this right after the deploys.
 * Idempotent: an account that exists is checked, not re-created.
 *
 * Not here: payment-rails `grant_engine` for the auction operator PDA. The
 * devnet rails build predates `GrantEngine` (#181) and needs a Squads
 * upgrade first.
 *
 *   PUBLISHER=~/.config/nundina/mark-engine-devnet/publisher.json \
 *     npx tsx scripts/devnet/engines.ts
 *   npx tsx scripts/devnet/engines.ts --status     # read-only: feed / venue / queue
 */
import { address, getAddressEncoder, getProgramDerivedAddress, type Address } from "@solana/kit";
import { rawAccount } from "../../src/chain.ts";
import { loadKeypair, makeCtx } from "../../src/ctx.ts";
import { loadIdl, makeCodec } from "../../src/idl.ts";
import { explorer, must, readDeployment, writeJson } from "./common.ts";

/** Devnet NPCS (#178). */
const NPCS = address("6vt1wHVQVuhnfJ3FtNbq4647mCjWUBmJCDABsYUgVKL4");

const mark = makeCodec(loadIdl("mark_engine"));
const auction = makeCodec(loadIdl("auction_engine"));
const queue = makeCodec(loadIdl("queue_claim"));
const SYSTEM = address("11111111111111111111111111111111");

const enc = getAddressEncoder();
const pda = async (program: Address, ...seeds: (string | Address)[]) =>
  (
    await getProgramDerivedAddress({
      programAddress: program,
      seeds: seeds.map((s) => (s.length >= 32 ? enc.encode(s as Address) : new TextEncoder().encode(s))),
    })
  )[0];

/**
 * Devnet feed parameters: the SRS 08 §3.2 weekly cadence (Warned after 8
 * days, Stale after 15), public prints, drills OFF so every NAV on the
 * devnet record comes from `publish_nav`.
 */
export const DEVNET_FEED_PARAMS = {
  warn_after: 8n * 86_400n,
  stale_after: 15n * 86_400n,
  warned_haircut_bps: 200n,
  stale_haircut_bps: 1_000n,
  print_policy: "Public",
  band_bps: 100n,
  min_print_volume: 1_000_000n,
  min_print_investors: 2,
  outlier_band_bps: 2_000n,
  drills_enabled: false,
};

const show = (v: unknown) =>
  JSON.stringify(v, (_, x) => (typeof x === "bigint" ? x.toString() : x instanceof Uint8Array ? Buffer.from(x).toString("hex") : x));

async function status(rpcUrl: string): Promise<void> {
  const ctx = await makeCtx({ rpcUrl, quiet: true });
  const feed = await rawAccount(ctx.rpc, await pda(mark.programId, "feed", NPCS));
  const venue = await rawAccount(ctx.rpc, await pda(auction.programId, "venue"));
  const q = await rawAccount(ctx.rpc, await pda(queue.programId, "queue", NPCS));
  const f = feed ? (mark.decodeAccount("Feed", feed.data) as Record<string, unknown>) : null;
  console.log("feed ", f ? show({ authority: f.authority, publishers: f.publishers, print_source: f.print_source, nav_pico: f.nav_pico, computed_ts: f.computed_ts, publications: f.publications, model_version: f.model_version, inputs_hash: f.inputs_hash }) : "missing");
  console.log("venue", venue ? show(auction.decodeAccount("Venue", venue.data)) : "missing");
  const qq = q ? (queue.decodeAccount("Queue", q.data) as Record<string, unknown>) : null;
  console.log("queue", qq ? show({ authority: qq.authority, attester: qq.attester, asset_mint: qq.asset_mint, usdc_mint: qq.usdc_mint }) : "missing");
}

async function main(): Promise<void> {
  const rpcUrl = process.env.RPC_URL ?? "https://api.devnet.solana.com";
  if (process.argv.includes("--status")) return status(rpcUrl);
  const d = readDeployment(rpcUrl);
  const authority = await loadKeypair(process.env.AUTHORITY ?? "~/.config/solana/turbine-devnet.json");
  if (authority.address !== d.authority) throw new Error(`AUTHORITY ${authority.address} is not the rails authority ${d.authority}`);
  const publisher = (await loadKeypair(process.env.PUBLISHER ?? "~/.config/nundina/mark-engine-devnet/publisher.json")).address;
  const ctx = await makeCtx({ rpcUrl, payer: authority });
  const usdc = address(d.usdcMint);
  const out: Record<string, unknown> = { programs: { mark: mark.programId, auction: auction.programId, queue: queue.programId }, steps: [] };
  const step = (name: string, sig?: string) => {
    (out.steps as unknown[]).push({ step: name, signature: sig, link: sig ? explorer(sig, rpcUrl) : undefined });
    console.log(`● ${name}${sig ? `\n    ${explorer(sig, rpcUrl)}` : ""}`);
  };

  // 1. feed
  const feed = await pda(mark.programId, "feed", NPCS);
  const print = await pda(auction.programId, "print");
  const feedRaw = await rawAccount(ctx.rpc, feed);
  if (!feedRaw) {
    step(
      `1 · mark-engine feed for NPCS created, publisher ${publisher}`,
      await must(ctx, "initialize_feed", [
        mark.ix("initialize_feed", { feed, asset_mint: NPCS, authority, system_program: SYSTEM }, { params: DEVNET_FEED_PARAMS }),
        mark.ix("set_publishers", { feed, authority }, { publishers: [publisher] }),
      ]),
    );
  } else {
    const f = mark.decodeAccount("Feed", feedRaw.data) as { authority: string };
    if (f.authority !== authority.address) throw new Error(`feed ${feed} exists with authority ${f.authority}: someone else initialized it`);
    step(`1 · feed ${feed} already exists (authority ok)`);
  }

  // 2. venue
  const venue = await pda(auction.programId, "venue");
  const venueRaw = await rawAccount(ctx.rpc, venue);
  if (!venueRaw) {
    step(
      "2 · auction-engine venue created",
      await must(ctx, "initialize_venue", [
        auction.ix(
          "initialize_venue",
          { venue, operator: await pda(auction.programId, "rails_operator"), authority, system_program: SYSTEM },
          { forced_sale_opener: SYSTEM },
        ),
      ]),
    );
  } else {
    const v = auction.decodeAccount("Venue", venueRaw.data) as { authority: string };
    if (v.authority !== authority.address) throw new Error(`venue ${venue} exists with authority ${v.authority}: someone else initialized it`);
    step(`2 · venue ${venue} already exists (authority ok)`);
  }

  // 3. print source
  const f = mark.decodeAccount("Feed", (await rawAccount(ctx.rpc, feed))!.data) as { print_source: string };
  if (f.print_source !== print) {
    step(`3 · feed print source = auction-engine print PDA ${print}`, await must(ctx, "set_print_source", [mark.ix("set_print_source", { feed, authority }, { print_source: print })]));
  } else step("3 · print source already set");

  // 4. queue
  const q = await pda(queue.programId, "queue", NPCS);
  const qRaw = await rawAccount(ctx.rpc, q);
  if (!qRaw) {
    step(
      "4 · queue-claim queue for NPCS created",
      await must(ctx, "init_queue", [
        queue.ix("init_queue", { queue: q, asset_mint: NPCS, usdc_mint: usdc, authority, system_program: SYSTEM }, { attester: authority.address }),
      ]),
    );
  } else {
    const qq = queue.decodeAccount("Queue", qRaw.data) as { authority: string };
    if (qq.authority !== authority.address) throw new Error(`queue ${q} exists with authority ${qq.authority}: someone else initialized it`);
    step(`4 · queue ${q} already exists (authority ok)`);
  }

  out.accounts = { feed, venue, print, queue: q, publisher };
  console.log(`\n✓ engines wired → ${writeJson(rpcUrl, "engines.json", out)}`);
}

main().catch((e) => {
  console.error("✗", e instanceof Error ? e.message : e);
  process.exit(1);
});
