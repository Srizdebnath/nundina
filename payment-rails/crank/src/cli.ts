/**
 * payment-rails crank CLI.
 *
 *   npm run crank -- refunds   --session <addr> [--escalate-to <usdc>]  # any wallet; past a
 *                                                          #   keeper's SLA, paid from its bond (#127)
 *   npm run crank -- keeper register --bond N --token <usdc>   # KEYPAIR = the keeper wallet
 *   npm run crank -- keeper top-up   --amount N --token <usdc>
 *   npm run crank -- keeper assign   --session <addr> --keeper <wallet>   # KEYPAIR = operator
 *   npm run crank -- keeper release  --session <addr>         # anyone
 *   npm run crank -- keeper withdraw --token <usdc>
 *   npm run crank -- seasoning --mint <gated mint>          # credit-gate merge_expired (#127)
 *   npm run crank -- phases --session <auction-engine session> # phase-deadline steps (#149)
 *   npm run crank -- distribute --pool <tranche pool> --cash N  # one coupon period, from KEYPAIR's USDC (#141)
 *   npm run crank -- run --config runner.json [--once]         # the crank-runner: every duty on a schedule (#141)
 *   npm run crank -- settle    --session <addr> --plan fills.json [--max-sends N]
 *   npm run crank -- finish    --session <addr>
 *   npm run crank -- reconcile --session <addr> | --facility <addr>
 *   npm run crank -- export    --out <dir> [--limit N]          # signed (#127)
 *   npm run crank -- export    --session <auction session> --out <dir>  # one session: events of all four programs + state snapshot (#141)
 *   npm run crank -- verify-export --dir <dir> [--signer <addr>] # keyless, offline
 *
 * Env: RPC_URL (default localhost), WS_URL, KEYPAIR (default
 * ~/.config/solana/id.json), EVIDENCE_KEYPAIR (signs exports; default
 * KEYPAIR), CRANK_KILL_FILE (runner sends nothing while it exists),
 * PRIORITY_FEE_MIN/MAX (µlamports/CU),
 * MAX_ATTEMPTS, RETRY_BASE_MS. Exit code 1 on any failed action or a
 * reconciliation that isn't clean.
 */
import { readFileSync } from "node:fs";
import { parseArgs } from "node:util";
import { address } from "@solana/kit";
import { finish, refundAll, settlePlan, type FillPlan, type Outcome } from "./crank.ts";
import { loadKeypair, makeCtx } from "./ctx.ts";
import { exportEvidence, verifyEvidence } from "./evidence.ts";
import { exportSessionEvidence } from "./session-evidence.ts";
import { assignKeeper, registerKeeper, releaseAssignment, topUpKeeper, withdrawKeeper } from "./keeper.ts";
import { distribute } from "./distribute.ts";
import { runPhases } from "./phases.ts";
import { loadRunnerConfig, run } from "./runner.ts";
import { seasonAll } from "./seasoning.ts";
import { formatReport, reconcileFacility, reconcileSession } from "./ledger.ts";

const { positionals, values } = parseArgs({
  allowPositionals: true,
  options: {
    session: { type: "string" },
    facility: { type: "string" },
    plan: { type: "string" },
    "max-sends": { type: "string" },
    out: { type: "string" },
    limit: { type: "string" },
    dir: { type: "string" },
    "escalate-to": { type: "string" },
    bond: { type: "string" },
    amount: { type: "string" },
    token: { type: "string" },
    keeper: { type: "string" },
    mint: { type: "string" },
    signer: { type: "string" },
    pool: { type: "string" },
    cash: { type: "string" },
    config: { type: "string" },
    once: { type: "boolean" },
  },
});

function need(name: keyof typeof values): string {
  const v = values[name];
  if (typeof v !== "string") throw new Error(`--${name} is required`);
  return v;
}

function summarize(outcomes: Outcome[]): boolean {
  for (const o of outcomes) console.log(`  ${o.result.padEnd(12)} ${o.target}${o.detail ? ` — ${o.detail}` : ""}`);
  const failed = outcomes.filter((o) => o.result === "failed").length;
  console.log(`${outcomes.length} targets, ${failed} failed`);
  return failed === 0;
}

export function parsePlan(json: string): FillPlan[] {
  const raw = JSON.parse(json) as {
    index: number;
    buyer: string;
    seller: string;
    qty: string | number;
    price: string | number;
    sellerUsdc?: string;
    buyerAsset?: string;
  }[];
  return raw.map((f) => ({
    index: f.index,
    buyer: address(f.buyer),
    seller: address(f.seller),
    qty: BigInt(f.qty),
    price: BigInt(f.price),
    sellerUsdc: f.sellerUsdc ? address(f.sellerUsdc) : undefined,
    buyerAsset: f.buyerAsset ? address(f.buyerAsset) : undefined,
  }));
}

async function main(): Promise<boolean> {
  const cmd = positionals[0];
  if (cmd === "verify-export") {
    // Needs neither a key nor an RPC: an auditor runs it on the files alone.
    const r = await verifyEvidence(need("dir"), values.signer);
    console.log(r.ok ? `evidence OK — signed by ${r.signer}` : `evidence REJECTED:\n  ${r.problems.join("\n  ")}`);
    return r.ok;
  }
  const ctx = await makeCtx();
  switch (cmd) {
    case "refunds":
      return summarize(await refundAll(ctx, address(need("session")), {
        escalateTo: values["escalate-to"] ? address(values["escalate-to"]) : undefined,
      }));
    case "keeper": {
      const sub = positionals[1];
      if (sub === "register") return summarize([await registerKeeper(ctx, BigInt(need("bond")), address(need("token")))]);
      if (sub === "top-up") return summarize([await topUpKeeper(ctx, BigInt(need("amount")), address(need("token")))]);
      if (sub === "assign") return summarize([await assignKeeper(ctx, address(need("session")), ctx.payer, address(need("keeper")))]);
      if (sub === "release") return summarize([await releaseAssignment(ctx, address(need("session")))]);
      if (sub === "withdraw") return summarize([await withdrawKeeper(ctx, address(need("token")))]);
      console.error("usage: crank keeper <register|top-up|assign|release|withdraw> — see src/cli.ts");
      return false;
    }
    case "seasoning":
      return summarize(await seasonAll(ctx, address(need("mint"))));
    case "phases":
      return summarize(await runPhases(ctx, address(need("session")), ctx.payer));
    case "distribute":
      return summarize([await distribute(ctx, address(need("pool")), BigInt(need("cash")), ctx.payer)]);
    case "run": {
      const outcomes = await run(ctx, loadRunnerConfig(need("config")), { once: values.once });
      return outcomes.every((o) => o.result !== "failed");
    }
    case "settle": {
      const plan = parsePlan(readFileSync(need("plan"), "utf8"));
      const maxSends = values["max-sends"] ? Number(values["max-sends"]) : undefined;
      return summarize(await settlePlan(ctx, address(need("session")), ctx.payer, plan, { maxSends }));
    }
    case "finish":
      return summarize([await finish(ctx, address(need("session")), ctx.payer)]);
    case "reconcile": {
      const r = values.facility
        ? await reconcileFacility(ctx.rpc, address(values.facility))
        : await reconcileSession(ctx.rpc, address(need("session")));
      console.log(formatReport(r));
      return r.ok;
    }
    case "export": {
      const signer = await loadKeypair(process.env.EVIDENCE_KEYPAIR ?? process.env.KEYPAIR ?? "~/.config/solana/id.json");
      const limit = values.limit ? Number(values.limit) : undefined;
      const m = values.session
        ? await exportSessionEvidence(ctx.rpc, need("out"), signer, address(values.session), { limit })
        : await exportEvidence(ctx.rpc, need("out"), signer, { limit });
      console.log(JSON.stringify(m, null, 2));
      return true;
    }
    default:
      console.error("usage: crank <refunds|settle|finish|reconcile|export|verify-export|keeper|seasoning|phases|distribute|run> [options] — see src/cli.ts");
      return false;
  }
}

if (import.meta.url === `file://${process.argv[1]}`) {
  main()
    .then((ok) => process.exit(ok ? 0 : 1))
    .catch((e) => {
      console.error(e instanceof Error ? e.message : e);
      process.exit(1);
    });
}
