import React from "react";
import { Addr, Banner, ErrorPanel, Stat, utc } from "@/components/app/ui";
import { AccessGuard } from "@/components/app/AccessGuard";
import { AdminActions } from "@/components/app/AdminActions";
import { loadAdmin, SQUADS_VAULT, type AdminState } from "@/lib/admin";
import { NPCS_MINT } from "@/lib/programs";
import { enumName } from "@/lib/prints";
import { fmt6, navToPrice } from "@/lib/session";
import type { Struct } from "@/lib/borsh";


function Who({ a, vault }: { a: string | null | undefined; vault: string }) {
  if (!a) return <span className="muted">—</span>;
  return (
    <>
      <Addr a={a} /> {a === vault ? <span className="pill ok">Squads vault</span> : <span className="pill warn">single key</span>}
    </>
  );
}

function AuthorityMap({ s, vault }: { s: AdminState; vault: string }) {
  const f = s.feed;
  const rows: [string, string | null | undefined, string][] = [
    ["payment-rails config authority", s.rails?.authority as string, "fees, breakers, operator, registrar, engine grants"],
    ["payment-rails operator", s.rails?.operator as string, "opens / fails / finishes cash sessions (or an engine with a grant)"],
    ["payment-rails registrar", s.rails?.registrar as string, "admits payout destinations"],
    ["credit-gate root", s.gateRoot?.authority as string, "initializes markets (CG-P2), survives an immutable program"],
    ["NPCS market config authority", s.gateMarket?.gate_config_authority as string, "policy, attesters, block-list, primary lots"],
    ["NPCS mark feed authority", f?.authority as string, "feed params, publishers, print source, suspend / resume"],
    ["auction venue authority", s.venue?.authority as string, "opens sessions, sets the forced-sale opener"],
    ["redemption queue authority", s.queue?.authority as string, "queue windows; attester signs claims"],
  ];
  return (
    <table className="t">
      <thead>
        <tr>
          <th>role</th>
          <th>held by</th>
          <th>can do</th>
        </tr>
      </thead>
      <tbody>
        {rows.map(([role, who, can]) => (
          <tr key={role}>
            <td>{role}</td>
            <td>
              <Who a={who} vault={vault} />
            </td>
            <td className="small">{can}</td>
          </tr>
        ))}
        {((f?.publishers as string[] | undefined) ?? []).slice(0, Number(f?.n_publishers ?? 0)).map((p) => (
          <tr key={p}>
            <td>NAV publisher</td>
            <td>
              <Who a={p} vault={vault} />
            </td>
            <td className="small">publish_nav only (ME-P1); cannot change params or governance</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

async function Admin() {
  let s: AdminState | null = null;
  let failure: string | null = null;
  try {
    s = await loadAdmin();
  } catch (e) {
    failure = (e as Error).message;
  }
  const vault = SQUADS_VAULT;
  if (!s) return <main className="wrap">{failure && <ErrorPanel title="RPC error">{failure}</ErrorPanel>}</main>;
  const f = s.feed;
  const fp = (f?.params ?? {}) as Struct;
  const granted = s.grants.some((g) => g.engine === s!.auctionOperator && (g.roles & 1) !== 0);
  return (
    <main className="wrap">
      <Banner />
      <h1>Admin</h1>
      <p className="muted small">
        Every authority on this cluster and what it can do (SRS 08 §8), read from the chain, and the admin actions. An action is signed
        here only by the key that holds the authority; when that is the Squads vault the page builds the proposal instead.
      </p>

      <h2>Programs</h2>
      <table className="t">
        <thead>
          <tr>
            <th>program</th>
            <th>id</th>
            <th>deployed</th>
            <th>upgrade authority</th>
            <th className="r">last deployed slot</th>
          </tr>
        </thead>
        <tbody>
          {s.programs.map((p) => (
            <tr key={p.name}>
              <td>{p.name}</td>
              <td>
                <Addr a={p.id} />
              </td>
              <td className={p.deployed ? "ok" : "muted"}>{p.deployed ? "yes" : "no"}</td>
              <td>{p.deployed ? p.upgradeAuthority ? <Who a={p.upgradeAuthority} vault={vault} /> : "immutable" : "—"}</td>
              <td className="r">{p.slot !== null ? String(p.slot) : "—"}</td>
            </tr>
          ))}
        </tbody>
      </table>

      <h2>Authority map</h2>
      <AuthorityMap s={s} vault={vault} />

      <h2>State</h2>
      <div className="grid">
        <Stat k="rails pause" v={s.rails ? (s.rails.paused ? "ENGAGED" : "off") : "—"} cls={s.rails?.paused ? "bad" : "ok"} />
        <Stat k="depeg breaker" v={s.rails ? (s.rails.depeg_breaker ? "ENGAGED" : "off") : "—"} cls={s.rails?.depeg_breaker ? "bad" : "ok"} />
        <Stat k="fee / reserve" v={s.rails ? `${String(s.rails.fee_bps)} / ${String(s.rails.reserve_bps)} bps` : "—"} />
        <Stat k="engine grants" v={s.grants.length ? s.grants.map((g) => `${g.engine.slice(0, 4)}…:${g.roles}`).join(" ") : "none"} />
        <Stat k="NPCS NAV" v={f ? fmt6(navToPrice(f.nav_pico as bigint, f.asset_decimals as number), 6) : "—"} />
        <Stat k="NAV computed" v={f ? utc(Number(f.computed_ts as bigint)) : "—"} />
        <Stat k="mark state / suspended" v={f ? `${enumName(f.last_state)} / ${f.suspended ? "yes" : "no"}` : "—"} cls={f?.suspended ? "bad" : ""} />
        <Stat k="print policy" v={f ? enumName(fp.print_policy) : "—"} />
      </div>

      <h2>Actions</h2>
      <AdminActions
        mint={NPCS_MINT}
        vault={vault}
        railsAuthority={(s.rails?.authority as string | undefined) ?? null}
        feeBps={Number(s.rails?.fee_bps ?? 0)}
        reserveBps={Number(s.rails?.reserve_bps ?? 0)}
        paused={!!s.rails?.paused}
        depeg={!!s.rails?.depeg_breaker}
        feedAuthority={(f?.authority as string | undefined) ?? null}
        feedSuspended={!!f?.suspended}
        gateAuthority={(s.gateMarket?.gate_config_authority as string | undefined) ?? null}
        auctionOperator={s.auctionOperator}
        auctionGranted={granted}
      />
      <p className="small muted">
        Not here, by design: SAS attestation issuance (the attester&apos;s own key, <code>ops/gate</code> sas.ts) and NAV publication
        (the publisher key, <code>services</code> nav-publisher): neither key belongs in a browser.
      </p>
    </main>
  );
}

export default function AdminPage() {
  return (
    <AccessGuard always what="The admin console" path="/app/admin">
      <Admin />
    </AccessGuard>
  );
}
