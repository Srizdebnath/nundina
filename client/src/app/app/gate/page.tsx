import React from "react";
import { address, isAddress } from "@solana/kit";
import { Addr, Banner, ErrorPanel, utc } from "@/components/app/ui";
import { WalletLookup } from "@/components/app/client";
import { gateStatus, hex, REJECT_TEXT, type GateStatus } from "@/lib/gate";
import { NPCS_MINT } from "@/lib/programs";
import { chainNow } from "@/lib/rpc";
import { fmt6 } from "@/lib/session";
import { connection } from "next/server";


function Check({ ok, label, children }: { ok: boolean | null; label: string; children?: React.ReactNode }) {
  return (
    <tr>
      <td style={{ width: 28 }} className={ok === null ? "muted" : ok ? "ok" : "bad"}>
        {ok === null ? "·" : ok ? "✓" : "✗"}
      </td>
      <td style={{ width: 230 }}>{label}</td>
      <td className="small">{children}</td>
    </tr>
  );
}

function Inputs({ s, now }: { s: GateStatus; now: number }) {
  const m = s.market!;
  const id = s.identity;
  const acc = s.accreditation;
  const horizon = now + Number(m.config.settlement_horizon_seconds as bigint);
  const attesters = (m.config.attesters as string[]).slice(0, m.config.attester_count as number);
  const inDate = (e: number) => e === 0 || e >= horizon;
  return (
    <table className="t">
      <tbody>
        <Check ok={s.blocked === null} label="block-list">
          {s.blocked ? `blocked at ${utc(Number(s.blocked.blocked_at as bigint))}` : "not listed"}
        </Check>
        <Check ok={id.att !== null && attesters.includes(id.att.signer)} label="identity attestation">
          <Addr a={id.address} />{" "}
          {id.att ? (
            <>
              signer <Addr a={id.att.signer} /> {attesters.includes(id.att.signer) ? "(listed attester)" : "(NOT a listed attester)"}
            </>
          ) : (
            "none at the canonical SAS address"
          )}
        </Check>
        <Check ok={id.att ? inDate(id.att.expiry) : null} label="identity expiry vs horizon">
          {id.att ? (id.att.expiry === 0 ? "never expires" : utc(id.att.expiry)) : "—"} · horizon {utc(horizon)}
        </Check>
        <Check
          ok={id.parsed ? m.jurisdictions.includes(id.parsed.jurisdiction) : null}
          label="jurisdiction"
        >
          {id.parsed ? id.parsed.jurisdiction : "—"} · policy allows {m.jurisdictions.join(", ") || "none"}
        </Check>
        <Check ok={id.parsed ? id.parsed.kycLevel >= (m.policy.min_kyc_level as number) : null} label="KYC level">
          {id.parsed ? id.parsed.kycLevel : "—"} · minimum {m.policy.min_kyc_level as number}
        </Check>
        <Check
          ok={!(m.policy.require_accreditation as boolean) ? true : acc.parsed ? acc.parsed.accredited && inDate(acc.att!.expiry) : null}
          label="accreditation"
        >
          {(m.policy.require_accreditation as boolean) ? "required" : "not required"} ·{" "}
          {acc.parsed ? (acc.parsed.accredited ? "accredited" : "not accredited") : "no attestation"}{" "}
          {acc.att && <>· expires {acc.att.expiry === 0 ? "never" : utc(acc.att.expiry)}</>}
        </Check>
        <Check
          ok={(m.policy.distinct_investors as number) < (m.config.holder_cap as number) || s.link !== null}
          label="holder governor"
        >
          {m.policy.distinct_investors as number} distinct investors · cap {m.config.holder_cap as number}
        </Check>
        <Check ok={s.link !== null} label="holder link">
          {s.link ? (
            <>
              investor {hex(s.link.investor_id as Uint8Array)} · validated {utc(Number(s.link.validated_at as bigint))}
            </>
          ) : (
            "not yet validated"
          )}
        </Check>
        <Check ok={s.tokenAccounts.length ? s.tokenAccounts.every((t) => !t.frozen) : null} label="token account state">
          {s.tokenAccounts.length === 0
            ? "no token account for this mint"
            : s.tokenAccounts.map((t) => (
                <span key={t.account}>
                  <Addr a={t.account} /> {t.frozen ? "frozen" : "thawed"} · {fmt6(t.amount)}{" "}
                </span>
              ))}
        </Check>
      </tbody>
    </table>
  );
}

export default async function GatePage({ searchParams }: { searchParams: Promise<{ wallet?: string; mint?: string }> }) {
  await connection();
  const q = await searchParams;
  const wallet = q.wallet && isAddress(q.wallet) ? address(q.wallet) : null;
  const mint = q.mint && isAddress(q.mint) ? address(q.mint) : NPCS_MINT;
  let status: GateStatus | null = null;
  let failure: string | null = null;
  if (wallet) {
    try {
      status = await gateStatus(mint, wallet);
    } catch (e) {
      failure = (e as Error).message;
    }
  }
  const now = wallet ? await chainNow() : 0;
  const d = status?.decision;
  return (
    <main className="wrap">
      <Banner />
      <h1>Gate check</h1>
      <p className="muted small">
        Runs <code>credit-gate</code>&apos;s real <code>validate</code> for a wallet on mint <Addr a={mint} /> as a
        simulation (nothing is signed or sent) and shows the program&apos;s own decision with the inputs it read.
      </p>
      <WalletLookup current={wallet} />
      {q.wallet && !wallet && <ErrorPanel title="Not an address">{q.wallet}</ErrorPanel>}
      {failure && <ErrorPanel title="RPC error">{failure}</ErrorPanel>}
      {status && d && (
        <>
          {d.error ? (
            <div className="verdict bad">
              <div className="head">REJECTED — {d.error}</div>
              <div className="small">The program returned a typed error instead of a decision.</div>
            </div>
          ) : d.reject ? (
            <div className="verdict bad">
              <div className="head">REJECTED — GateReject::{d.reject}</div>
              <div className="small" style={{ marginTop: 6 }}>
                {REJECT_TEXT[d.reject] ?? ""} Nothing was admitted; the token account stays frozen.
              </div>
            </div>
          ) : (
            <div className="verdict ok">
              <div className="head">ADMITTED</div>
              <div className="small" style={{ marginTop: 6 }}>
                <code>validate</code> approves this wallet ({d.distinctInvestors} distinct investors after). Token ACL&apos;s
                permissionless thaw will succeed for its token accounts.
              </div>
            </div>
          )}
          {status.market ? (
            <>
              <h2>Inputs the gate read</h2>
              <Inputs s={status} now={now} />
            </>
          ) : null}
          {status.lots && (
            <>
              <h2>Lot ledger (CG-P3)</h2>
              <p className="small muted">
                free (seasoned) {fmt6(status.lots.freeQty)} · {status.lots.lots.length} seasoning lot(s) —{" "}
                <a href={`/app/portfolio?wallet=${status.wallet}`}>see portfolio</a>
              </p>
            </>
          )}
          <h2>Simulation logs</h2>
          <p className="small muted">fee payer for the simulation: {d.payer ? <Addr a={d.payer} /> : "—"}</p>
          <pre className="logs">{d.logs.join("\n") || "—"}</pre>
        </>
      )}
    </main>
  );
}
