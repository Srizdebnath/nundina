import React from "react";
import Link from "next/link";
import { Banner, ErrorPanel, utc } from "@/components/app/ui";
import { AutoRefresh } from "@/components/app/client";
import { PROGRAMS } from "@/lib/programs";
import { fmt6, listSessions } from "@/lib/session";
import { connection } from "next/server";


export default async function SessionsPage() {
  await connection();
  let sessions: Awaited<ReturnType<typeof listSessions>> = [];
  let failure: string | null = null;
  try {
    sessions = await listSessions();
  } catch (e) {
    failure = (e as Error).message;
  }
  return (
    <main className="wrap">
      <AutoRefresh seconds={10} />
      <Banner />
      <h1>Auction sessions</h1>
      <p className="small muted">Every session account owned by auction-engine {PROGRAMS.auction} on this cluster.</p>
      {failure && <ErrorPanel title="RPC error">{failure}</ErrorPanel>}
      {!failure && sessions.length === 0 && (
        <p className="muted">No sessions on this cluster (auction-engine has none, or is not deployed here).</p>
      )}
      {sessions.length > 0 && (
        <table className="t">
          <thead>
            <tr>
              <th>#</th>
              <th>phase</th>
              <th>commit closes</th>
              <th className="r">orders</th>
              <th className="r">clear</th>
              <th className="r">volume</th>
              <th className="r">fills settled</th>
            </tr>
          </thead>
          <tbody>
            {sessions.map((s) => (
              <tr key={s.address}>
                <td>
                  <Link href={`/app/sessions/${s.address}`}>#{String(s.id)}</Link>
                </td>
                <td>{s.phase}</td>
                <td>{utc(Number(s.params.commit_end as bigint))}</td>
                <td className="r">{s.orders.length}</td>
                <td className="r">{s.clearPrice > 0n ? fmt6(s.clearPrice, 4) : "—"}</td>
                <td className="r">{s.clearedVolume > 0n ? fmt6(s.clearedVolume, 2) : "—"}</td>
                <td className="r">
                  {s.fillsSettled} / {s.fills.length}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </main>
  );
}
