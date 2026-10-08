import React from "react";
import Link from "next/link";
import { Addr, Banner, ErrorPanel, Stat, utc } from "@/components/ui";
import { codecs, CLUSTER, NPCS_MINT, pdas, PROGRAMS, RPC_URL } from "@/lib/programs";
import { getAccounts } from "@/lib/rpc";
import { fmt6, listSessions, navToPrice } from "@/lib/session";
import type { Struct } from "@/lib/borsh";

export const dynamic = "force-dynamic";

const NAMES: Record<keyof typeof PROGRAMS, string> = {
  gate: "credit-gate",
  rails: "payment-rails",
  auction: "auction-engine",
  mark: "mark-engine",
  queue: "queue-claim",
  facility: "liquidity-facility",
  tranche: "tranche-engine",
  treasury: "npcs-treasury",
};

export default async function Home() {
  const keys = Object.keys(PROGRAMS) as (keyof typeof PROGRAMS)[];
  let failure: string | null = null;
  let deployed: boolean[] = [];
  let config: Struct | null = null;
  let policy: Struct | null = null;
  let feed: Struct | null = null;
  let sessions = 0;
  try {
    const [configA, policyA, feedA] = await Promise.all([
      pdas.gateConfig(NPCS_MINT),
      pdas.gatePolicy(NPCS_MINT),
      pdas.feed(NPCS_MINT),
    ]);
    const accts = await getAccounts([...keys.map((k) => PROGRAMS[k]), configA, policyA, feedA]);
    deployed = keys.map((_, i) => accts[i] !== null);
    const [c, p, f] = accts.slice(keys.length);
    config = c?.owner === PROGRAMS.gate ? codecs.gate.decodeAccount("MarketConfig", c.data) : null;
    policy = p?.owner === PROGRAMS.gate ? codecs.gate.decodeAccount("MarketPolicy", p.data) : null;
    feed = f?.owner === PROGRAMS.mark ? codecs.mark.decodeAccount("Feed", f.data) : null;
    if (deployed[keys.indexOf("auction")]) sessions = (await listSessions()).length;
  } catch (e) {
    failure = (e as Error).message;
  }
  return (
    <main className="wrap">
      <Banner />
      <h1>NUNDINA Pilot</h1>
      <p className="muted small">
        The pricing, queue &amp; risk-transfer engine for gated, NAV-priced private credit. Everything below is read live
        from <b>{CLUSTER}</b> ({RPC_URL}).
      </p>
      {failure && <ErrorPanel title="RPC error">{failure}</ErrorPanel>}

      <h2>Demo screens</h2>
      <div className="grid">
        <Link href="/sessions" className="panel">
          <b>Sessions</b>
          <div className="small muted">commit → reveal → uniform clear → DvP settle → print</div>
        </Link>
        <Link href="/portfolio" className="panel">
          <b>Portfolio</b>
          <div className="small muted">lots + seasoning countdown, claims, advances, orders</div>
        </Link>
        <Link href="/gate" className="panel">
          <b>Gate check</b>
          <div className="small muted">credit-gate&apos;s own verdict for any wallet, with the typed rejection</div>
        </Link>
      </div>

      <h2>NPCS market</h2>
      <p className="small muted">
        mint <Addr a={NPCS_MINT} />
      </p>
      <div className="grid">
        <Stat k="gate market" v={config ? "configured" : "none"} cls={config ? "ok" : "bad"} />
        <Stat
          k="distinct investors / cap"
          v={config && policy ? `${policy.distinct_investors as number} / ${config.holder_cap as number}` : "—"}
        />
        <Stat k="seasoning" v={config ? `${Number(config.seasoning_seconds as bigint) / 86_400} days` : "—"} />
        <Stat k="mark feed" v={feed ? String(feed.last_state) : "not on this cluster"} cls={feed ? "" : "muted"} />
        <Stat k="NAV" v={feed ? fmt6(navToPrice(feed.nav_pico as bigint, feed.asset_decimals as number), 4) : "—"} />
        <Stat k="NAV effective" v={feed ? utc(Number(feed.effective_ts as bigint)) : "—"} />
        <Stat k="auction sessions" v={String(sessions)} />
      </div>

      <h2>Programs on {CLUSTER}</h2>
      <table className="t">
        <tbody>
          {keys.map((k, i) => (
            <tr key={k}>
              <td style={{ width: 200 }}>{NAMES[k]}</td>
              <td>
                <Addr a={PROGRAMS[k]} />
              </td>
              <td className={deployed[i] ? "ok" : "muted"}>{deployed[i] ? "deployed" : "not deployed here"}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </main>
  );
}
