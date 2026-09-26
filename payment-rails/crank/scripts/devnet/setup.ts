/**
 * R5 one-time setup on a cluster where the program is deployed:
 * fee-receiver + reserve accounts, `initialize_config` (upgrade authority
 * only; the settlement mint is **permanent** for this deployment), pilot
 * fees. Idempotent: re-running verifies instead of re-initialising.
 *
 *   RPC_URL=https://api.devnet.solana.com AUTHORITY=~/.config/solana/id.json \
 *     npx tsx scripts/devnet/setup.ts
 *
 * USDC_MINT overrides the settlement mint (default: Circle devnet USDC).
 */
import { address, getAddressEncoder, getProgramDerivedAddress } from "@solana/kit";
import { fetchDecoded, rawAccount, tokenProgramOf } from "../../src/chain.ts";
import { loadKeypair, makeCtx } from "../../src/ctx.ts";
import { ix, PROGRAM_ID, type Struct } from "../../src/idl.ts";
import { configPda } from "../../src/pda.ts";
import { CIRCLE_DEVNET_USDC, clusterLabel, ensureAta, ensureSeedTokenAccount, must, writeJson, type Deployment } from "./common.ts";

const LOADER_V3 = address("BPFLoaderUpgradeab1e11111111111111111111111");
const FEE_BPS = 10;
const RESERVE_BPS = 5;

async function main(): Promise<void> {
  const rpcUrl = process.env.RPC_URL ?? "https://api.devnet.solana.com";
  const authority = await loadKeypair(process.env.AUTHORITY ?? "~/.config/solana/id.json");
  const ctx = await makeCtx({ rpcUrl, payer: authority });
  const usdcMint = process.env.USDC_MINT ? address(process.env.USDC_MINT) : CIRCLE_DEVNET_USDC;
  console.log(`cluster ${clusterLabel(rpcUrl)} · program ${PROGRAM_ID} · authority ${authority.address}`);

  const program = await rawAccount(ctx.rpc, PROGRAM_ID);
  if (!program || program.owner !== LOADER_V3) throw new Error("program not deployed here — run deploy.sh first");
  const [programData] = await getProgramDerivedAddress({ programAddress: LOADER_V3, seeds: [getAddressEncoder().encode(PROGRAM_ID)] });

  const usdcTokenProgram = await tokenProgramOf(ctx.rpc, usdcMint);
  const feeReceiver = await ensureAta(ctx, authority.address, usdcMint, usdcTokenProgram);
  const reserve = await ensureSeedTokenAccount(ctx, authority, "rails-reserve", usdcMint, usdcTokenProgram);
  const config = await configPda();

  const existing = await fetchDecoded<Struct>(ctx.rpc, "RailsConfig", config);
  if (!existing) {
    const sig = await must(ctx, "initialize_config", [
      ix("initialize_config", {
        config, usdc_mint: usdcMint, fee_receiver: feeReceiver, reserve, authority,
        program: PROGRAM_ID, program_data: programData, token_program: usdcTokenProgram,
      }),
    ]);
    console.log(`✓ config initialised (${sig})`);
  } else {
    const same =
      existing.usdc_mint === usdcMint && existing.fee_receiver === feeReceiver && existing.reserve === reserve && existing.authority === authority.address;
    if (!same) {
      throw new Error(
        `config already initialised with different parameters (usdc ${existing.usdc_mint}, fee ${existing.fee_receiver}, reserve ${existing.reserve}, authority ${existing.authority}) — it is immutable per deployment`,
      );
    }
    console.log("✓ config already initialised with these parameters");
  }

  const cfg = (await fetchDecoded<Struct>(ctx.rpc, "RailsConfig", config))!;
  if (cfg.fee_bps !== FEE_BPS || cfg.reserve_bps !== RESERVE_BPS) {
    await must(ctx, "set_fees", [ix("set_fees", { config, authority }, { fee_bps: FEE_BPS, reserve_bps: RESERVE_BPS })]);
  }

  const d: Deployment = {
    cluster: clusterLabel(rpcUrl), rpcUrl, programId: PROGRAM_ID, authority: authority.address, config,
    usdcMint, usdcTokenProgram, feeReceiver, reserve, feeBps: FEE_BPS, reserveBps: RESERVE_BPS,
  };
  console.log(`✓ deployment state → ${writeJson(rpcUrl, "deployment.json", d)}`);
  console.log(JSON.stringify(d, null, 2));
}

main().catch((e) => {
  console.error(e instanceof Error ? e.message : e);
  process.exit(1);
});
