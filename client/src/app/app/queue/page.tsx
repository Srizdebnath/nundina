import React from "react";
import { address, isAddress } from "@solana/kit";
import { Addr, Banner, ErrorPanel, Stat, utc } from "@/components/app/ui";
import { AutoRefresh, WalletLookup } from "@/components/app/client";
import { QueueActions } from "@/components/app/FlowActions";
import { loadQueue, mintInfos, type QueueState } from "@/lib/flows-read";
import { NPCS_MINT, PROGRAMS } from "@/lib/programs";
import { fmt6 } from "@/lib/session";
import type { Struct } from "@/lib/borsh";
import { connection } from "next/server";


const stateName = (v: unknown) => (typeof v === "string" ? v : (Object.keys(v as Struct)[0] ?? "?"));

/** What the attester has (or has not yet) said about a request (QC-P1/P2). */
function Attest({ r }: { r: Struct }) {
  const s = stateName(r.state);
  if (s === "Requested") return <span className="warn">awaiting attester</span>;
  if (s === "Cancelled") return <span className="muted">cancelled</span>;
  const schedule = r.schedule as Struct[];
  return (
    <span className="ok" title={schedule.map((f) => `period ${f.period as number}: ${fmt6(f.qty as bigint)}`).join("\n")}>
      attested · rank {r.rank as number} · {schedule.length} scheduled fill{schedule.length === 1 ? "" : "s"}
    </span>
  );
}

export default async function QueuePage({ searchParams }: { searchParams: Promise<{ wallet?: string }> }) {
  await connection();
  const q = await searchParams;
  const wallet = q.wallet && isAddress(q.wallet) ? address(q.wallet) : null;
  let s: QueueState | null = null;
  let mint: Awaited<ReturnType<typeof mintInfos>>[number] = null;
  let failure: string | null = null;
  try {
    [s, [mint]] = await Promise.all([loadQueue(NPCS_MINT, wallet), mintInfos([NPCS_MINT])]);
  } catch (e) {
    failure = (e as Error).message;
  }
  return (
    <main className="wrap">
      <Banner />
      <h1>Redemption queue</h1>
      <p className="muted small">
        queue-claim (QC-P1/P2): a holder escrows NPCS in a redemption request; the attester (issuer / transfer agent) confirms
        it with a queue rank and an expected fill schedule and mints the frozen claim token.
      </p>
      {failure && <ErrorPanel title="RPC error">{failure}</ErrorPanel>}
      {s && !s.deployed && (
        <ErrorPanel title="queue-claim is not deployed on this cluster">
          Program <Addr a={PROGRAMS.queue} /> has no account here.
        </ErrorPanel>
      )}
      {s?.deployed && !s.queue && (
        <ErrorPanel title="No queue for this asset">
          <Addr a={s.address} /> is not initialized: <code>init_queue</code> (the program&apos;s upgrade authority) creates it.
        </ErrorPanel>
      )}
      {s?.queue && (
        <>
          <AutoRefresh seconds={20} />
          <div className="grid">
            <Stat k="queue" v={<Addr a={s.address} />} />
            <Stat k="requests so far" v={String(s.queue.requests as bigint)} />
            <Stat k="attester" v={<Addr a={s.queue.attester as string} />} />
          </div>
          <h2>Request a redemption</h2>
          {mint ? (
            <QueueActions queue={s.address} mint={NPCS_MINT} decimals={mint.decimals} assetTokenProgram={mint.program} />
          ) : (
            <p className="small bad">The NPCS mint is not on this cluster.</p>
          )}
          <h2>Requests</h2>
          <WalletLookup current={wallet} />
          {wallet && s.requests.length === 0 && <p className="muted small">No requests from this wallet.</p>}
          {s.requests.length > 0 && (
            <table className="t">
              <thead>
                <tr>
                  <th>request</th>
                  <th className="r">qty</th>
                  <th>window</th>
                  <th>requested</th>
                  <th>state</th>
                  <th>attestation</th>
                  <th className="r">filled</th>
                  <th className="r">cash</th>
                </tr>
              </thead>
              <tbody>
                {s.requests.map(({ address: a, request: r }) => (
                  <tr key={a}>
                    <td>
                      <Addr a={a} /> #{String(r.id)}
                    </td>
                    <td className="r">{fmt6(r.qty as bigint)}</td>
                    <td>{utc(Number(r.window_date as bigint)).slice(0, 10)}</td>
                    <td>{utc(Number(r.request_ts as bigint))}</td>
                    <td>{stateName(r.state)}</td>
                    <td>
                      <Attest r={r} />
                    </td>
                    <td className="r">{fmt6(r.filled_qty as bigint)}</td>
                    <td className="r">{fmt6(r.cash_received as bigint)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </>
      )}
    </main>
  );
}
