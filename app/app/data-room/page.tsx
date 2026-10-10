import React from "react";
import Link from "next/link";
import { Addr, Banner, ErrorPanel, Stat, utc } from "@/components/ui";
import { AccessGuard } from "@/components/AccessGuard";
import { disclose, loadPrints, type PrintRow } from "@/lib/prints";
import { fmt6, navToPrice } from "@/lib/session";
import { viewer } from "@/lib/access";

export const dynamic = "force-dynamic";

const CLASS_TEXT: Record<string, string> = {
  Public: "price and volume public",
  Banded: "price rounded down to the band, volume public",
  AggregateOnly: "volume only; no price outside the data room",
  DataRoom: "nothing outside the data room",
};

function discountBps(r: PrintRow): number | null {
  const nav = navToPrice(r.navPico, r.assetDecimals);
  return nav > 0n ? Number(((nav - r.price) * 10_000n) / nav) : null;
}

async function DataRoom() {
  const me = await viewer();
  let rows: PrintRow[] = [];
  let failure: string | null = null;
  try {
    rows = await loadPrints();
  } catch (e) {
    failure = (e as Error).message;
  }
  const eligible = rows.filter((r) => r.eligibility === "Eligible");
  const vol = eligible.reduce((s, r) => s + r.volume, 0n);
  const vwap = vol > 0n ? eligible.reduce((s, r) => s + r.price * r.volume, 0n) / vol : 0n;
  return (
    <main className="wrap">
      <Banner />
      <h1>Data room</h1>
      <p className="muted small">
        Every mark-engine print in full, next to what its market&apos;s class (ME-P6) discloses outside this room. Viewer{" "}
        {me ? <Addr a={me} /> : "—"} (allowlisted). Prints are synthetic-labeled demo data (doc 07 print hygiene) and excluded from any
        dataset claim.
      </p>
      {failure && <ErrorPanel title="RPC error">{failure}</ErrorPanel>}
      <div className="grid">
        <Stat k="prints" v={String(rows.length)} />
        <Stat k="eligible (feed the implied mark)" v={String(eligible.length)} />
        <Stat k="eligible volume" v={vol > 0n ? fmt6(vol, 2) : "—"} />
        <Stat k="eligible VWAP" v={vwap > 0n ? fmt6(vwap, 4) : "—"} />
      </div>
      <h2>Prints</h2>
      {rows.length === 0 ? (
        <p className="muted small">No prints on this cluster yet: a session records one when it finishes.</p>
      ) : (
        <table className="t">
          <thead>
            <tr>
              <th>recorded</th>
              <th>session</th>
              <th>class</th>
              <th className="r">price</th>
              <th className="r">volume</th>
              <th className="r">investors</th>
              <th className="r">vs NAV</th>
              <th>eligibility</th>
              <th>outside the room</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((r) => {
              const d = disclose(r.policy, r.price, r.volume, r.bandBps);
              const disc = discountBps(r);
              return (
                <tr key={r.address}>
                  <td>{utc(r.recordedAt)}</td>
                  <td>
                    <Link href={`/sessions/${r.session}`}>{r.session.slice(0, 4)}…</Link>
                  </td>
                  <td title={CLASS_TEXT[r.policy]}>{r.policy}</td>
                  <td className="r">{fmt6(r.price, 4)}</td>
                  <td className="r">{fmt6(r.volume, 2)}</td>
                  <td className="r">{r.investors}</td>
                  <td className="r">{disc !== null ? `${(-disc / 100).toFixed(2)}%` : "—"}</td>
                  <td className={r.eligibility === "Eligible" ? "ok" : "warn"}>
                    {r.eligibility}
                    {r.forcedSale ? " (forced sale)" : ""}
                  </td>
                  <td className="small muted">
                    price {d.price !== null ? fmt6(d.price, 4) : "withheld"} · volume {d.volume !== null ? fmt6(d.volume, 2) : "withheld"}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}
      <p className="small muted">
        &quot;vs NAV&quot; compares with the feed&apos;s current NAV, not the NAV at the time of the print (that one is in the session
        evidence bundle).
      </p>
    </main>
  );
}

export default function DataRoomPage() {
  return (
    <AccessGuard always what="The data room" path="/data-room">
      <DataRoom />
    </AccessGuard>
  );
}
