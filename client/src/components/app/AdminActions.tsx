"use client";
/**
 * Admin actions. Each card names the account that may sign it, read from
 * the chain. If the connected wallet is that account the card signs and
 * sends; if it is the Squads vault the card exports the instruction for a
 * vault proposal (SRS 08 §7: admin pages go through multisig proposals);
 * otherwise it says who must act.
 */
import React, { useState } from "react";
import { address, isAddress, type Address, type Instruction } from "@solana/kit";
import { adminIx, noteHash, proposalJson, ROLE } from "@/lib/admin-tx";
import { explorer } from "@/lib/programs";
import { useWallet } from "@/lib/wallet";

export interface AdminProps {
  mint: string;
  vault: string;
  railsAuthority: string | null;
  feeBps: number;
  reserveBps: number;
  paused: boolean;
  depeg: boolean;
  feedAuthority: string | null;
  feedSuspended: boolean;
  gateAuthority: string | null;
  auctionOperator: string;
  auctionGranted: boolean;
}

function Card(props: {
  title: string;
  authority: string | null;
  vault: string;
  children?: React.ReactNode;
  build: (authority: Address) => Promise<Instruction>;
  label: string;
}) {
  const { account, send } = useWallet();
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<{ ok: boolean; text: string; sig?: string } | null>(null);
  const [json, setJson] = useState<string | null>(null);
  if (!props.authority) {
    return (
      <div className="panel">
        <h3>{props.title}</h3>
        <p className="small muted">The account this action administers does not exist on this cluster.</p>
      </div>
    );
  }
  const authority = address(props.authority);
  const isVault = authority === props.vault;
  const mine = account === authority;
  const act = async () => {
    setBusy(true);
    setMsg(null);
    setJson(null);
    try {
      const ix = await props.build(authority);
      if (mine) setMsg({ ok: true, text: "sent", sig: await send([ix]) });
      else setJson(proposalJson(ix));
    } catch (e) {
      setMsg({ ok: false, text: (e as Error).message });
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="panel">
      <h3 style={{ marginTop: 0 }}>{props.title}</h3>
      <p className="small muted">
        authority <code title={authority}>{authority.slice(0, 4)}…{authority.slice(-4)}</code>
        {isVault ? " (Squads vault: 2-of-2, 48 h timelock)" : " (single key)"}
      </p>
      {props.children}
      <p className="small">
        {mine ? (
          <button disabled={busy} onClick={() => void act()}>
            {props.label}: sign &amp; send
          </button>
        ) : isVault ? (
          <button className="ghost" disabled={busy} onClick={() => void act()}>
            {props.label}: build the vault proposal
          </button>
        ) : (
          <span className="muted">Connect the authority&apos;s wallet to act.</span>
        )}
      </p>
      {json && (
        <>
          <p className="small">
            Propose this as a vault transaction (Squads UI transaction builder, or <code>ops/squads</code>); it executes after both members
            approve and the timelock passes.
          </p>
          <pre className="small" style={{ whiteSpace: "pre-wrap", wordBreak: "break-all" }}>
            {json}
          </pre>
          <button className="ghost" onClick={() => void navigator.clipboard.writeText(json)}>
            copy
          </button>
        </>
      )}
      {msg && (
        <p className={`small ${msg.ok ? "ok" : "bad"}`}>
          {msg.text}{" "}
          {msg.sig && (
            <a href={explorer("tx", msg.sig)} target="_blank" rel="noreferrer">
              {msg.sig.slice(0, 12)}…
            </a>
          )}
        </p>
      )}
    </div>
  );
}

export function AdminActions(p: AdminProps) {
  const mint = address(p.mint);
  const [note, setNote] = useState("");
  const [fee, setFee] = useState(String(p.feeBps));
  const [reserve, setReserve] = useState(String(p.reserveBps));
  const [wallet, setWallet] = useState("");
  const needNote = async () => {
    if (!note.trim()) throw new Error("write the reason: its sha256 goes on-chain as evidence");
    return noteHash(note.trim());
  };
  const target = () => {
    if (!isAddress(wallet.trim())) throw new Error("wallet must be a base58 address");
    return address(wallet.trim());
  };
  const int = (s: string) => {
    if (!/^\d+$/.test(s.trim())) throw new Error(`not a whole number of bps: ${s}`);
    return Number(s.trim());
  };
  return (
    <>
      <div className="panel">
        <label className="small">
          reason / evidence note (hashed into breaker, suspend and block actions)
          <input value={note} onChange={(e) => setNote(e.target.value)} size={70} placeholder="e.g. drill 2026-10-08: AC-11 kill switch" />
        </label>
      </div>
      <div className="grid" style={{ gridTemplateColumns: "repeat(auto-fill, minmax(340px, 1fr))" }}>
        <Card
          title={`Rails pause: ${p.paused ? "ENGAGED" : "off"}`}
          authority={p.railsAuthority}
          vault={p.vault}
          label={p.paused ? "release pause" : "engage pause"}
          build={async (a) => adminIx.setBreaker(a, "Pause", !p.paused, await needNote())}
        >
          <p className="small">Halts new deposits, sessions and settlement; refunds stay open (AE-P10).</p>
        </Card>
        <Card
          title={`Depeg breaker: ${p.depeg ? "ENGAGED" : "off"}`}
          authority={p.railsAuthority}
          vault={p.vault}
          label={p.depeg ? "release depeg breaker" : "engage depeg breaker"}
          build={async (a) => adminIx.setBreaker(a, "Depeg", !p.depeg, await needNote())}
        />
        <Card
          title="Fees and reserve"
          authority={p.railsAuthority}
          vault={p.vault}
          label="set fees"
          build={(a) => adminIx.setFees(a, int(fee), int(reserve))}
        >
          <div className="row small">
            <label>
              fee bps <input value={fee} onChange={(e) => setFee(e.target.value)} size={5} />
            </label>
            <label>
              reserve bps <input value={reserve} onChange={(e) => setReserve(e.target.value)} size={5} />
            </label>
          </div>
          <p className="small muted">The program caps the fee at 0 (doc 07 AE-3) and the reserve at 200 bps.</p>
        </Card>
        <Card
          title={`Auction engine as rails operator: ${p.auctionGranted ? "granted" : "not granted"}`}
          authority={p.railsAuthority}
          vault={p.vault}
          label="grant ROLE_SESSIONS"
          build={(a) => adminIx.grantEngine(a, address(p.auctionOperator), ROLE.sessions)}
        >
          <p className="small">
            Admits auction-engine&apos;s operator PDA <code>{p.auctionOperator.slice(0, 4)}…</code> to open rails cash sessions (#136).
          </p>
        </Card>
        <Card
          title={`NPCS mark feed: ${p.feedSuspended ? "SUSPENDED" : "live"}`}
          authority={p.feedAuthority}
          vault={p.vault}
          label={p.feedSuspended ? "resume feed" : "suspend feed"}
          build={async (a) => (p.feedSuspended ? adminIx.resumeFeed(a, mint) : adminIx.suspendFeed(a, mint, await needNote()))}
        >
          <p className="small">Suspended: no new advances, no liquidations on the mark (ME-P7).</p>
        </Card>
        <Card
          title="Gate block-list"
          authority={p.gateAuthority}
          vault={p.vault}
          label="block wallet"
          build={async (a) => adminIx.blockWallet(a, mint, target(), await needNote())}
        >
          <label className="small">
            wallet <input value={wallet} onChange={(e) => setWallet(e.target.value)} size={46} />
          </label>
        </Card>
        <Card
          title="Gate unblock"
          authority={p.gateAuthority}
          vault={p.vault}
          label="unblock wallet"
          build={(a) => adminIx.unblockWallet(a, mint, target())}
        >
          <p className="small muted">Uses the wallet field of the block-list card.</p>
        </Card>
      </div>
    </>
  );
}
