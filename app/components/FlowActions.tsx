"use client";
/**
 * The participant writes of /queue, /facility, /tranches and /treasury
 * (#202), signed by the connected wallet. Builders are `lib/flows.ts`; the
 * page passes what it read from the chain, and anything that moves between
 * render and click (the queue's next request id, token accounts) is read
 * again here right before the transaction is built.
 */
import React, { useState } from "react";
import { address, type Address, type Instruction } from "@solana/kit";
import { ata, codecs, explorer } from "@/lib/programs";
import { getAccount, getTokenHoldings, type TokenHolding } from "@/lib/rpc";
import { fmt6, parseUnits } from "@/lib/session";
import {
  addCollateralIx,
  createAtaIx,
  owedAt,
  pledgeAndAdvanceIx,
  releaseIx,
  repayIx,
  requestRedemptionIx,
  trancheDepositIx,
  treasuryDepositIx,
  treasuryRedeemIx,
  type FacilityRef,
} from "@/lib/flows";
import { useWallet } from "@/lib/wallet";

type Msg = { ok: boolean; text: string; sig?: string } | null;

function useRun() {
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<Msg>(null);
  const run = async (fn: () => Promise<string>, ok: string) => {
    setBusy(true);
    setMsg(null);
    try {
      setMsg({ ok: true, text: ok, sig: await fn() });
    } catch (e) {
      setMsg({ ok: false, text: (e as Error).message });
    } finally {
      setBusy(false);
    }
  };
  return { busy, msg, run };
}

function TxMsg({ msg }: { msg: Msg }) {
  if (!msg) return null;
  return (
    <p className={`small ${msg.ok ? "ok" : "bad"}`}>
      {msg.text}{" "}
      {msg.sig && (
        <a href={explorer("tx", msg.sig)} target="_blank" rel="noreferrer">
          {msg.sig.slice(0, 12)}…
        </a>
      )}
    </p>
  );
}

const largest = (hs: TokenHolding[], mint: string) =>
  hs.filter((h) => h.mint === mint).sort((a, b) => (a.amount > b.amount ? -1 : 1))[0] ?? null;

/** The wallet's account of a default-frozen asset it is about to move: present and thawed. */
function movable(hs: TokenHolding[], mint: string, what: string, need: bigint): TokenHolding {
  const h = largest(hs, mint);
  if (!h) throw new Error(`no ${what} token account in this wallet`);
  if (h.frozen) throw new Error(`your ${what} account is frozen: the gate thaws it (Token ACL) once credit-gate admits the wallet`);
  if (h.amount < need) throw new Error(`not enough ${what}: ${fmt6(h.amount)} held`);
  return h;
}

/** An existing account for `mint`, or the ATA plus the instruction that opens it. */
async function accountFor(hs: TokenHolding[], owner: Address, mint: Address, program: Address): Promise<{ account: Address; pre: Instruction[] }> {
  const h = largest(hs, mint);
  if (h) return { account: h.account, pre: [] };
  return { account: await ata(owner, mint, program), pre: [await createAtaIx(owner, owner, mint, program)] };
}

function Field({ label, value, set, size = 12, type = "text" }: { label: string; value: string; set: (v: string) => void; size?: number; type?: string }) {
  return (
    <label>
      {label} <input type={type} value={value} onChange={(e) => set(e.target.value)} size={size} />
    </label>
  );
}

const connect = <p className="small muted">Connect a wallet to sign.</p>;

// ── queue-claim ─────────────────────────────────────────────────────────

/** First day of next month, UTC, as yyyy-mm-dd (the default redemption window). */
function nextWindow(): string {
  const d = new Date();
  return new Date(Date.UTC(d.getUTCFullYear(), d.getUTCMonth() + 1, 1)).toISOString().slice(0, 10);
}

export function QueueActions(p: { queue: string; mint: string; decimals: number; assetTokenProgram: string }) {
  const { account, send } = useWallet();
  const { busy, msg, run } = useRun();
  const [qty, setQty] = useState("1");
  const [win, setWin] = useState(nextWindow);
  if (!account) return connect;
  const submit = () =>
    run(async () => {
      const q = parseUnits(qty, p.decimals);
      if (q <= 0n) throw new Error("quantity must be positive");
      const date = Date.parse(`${win}T00:00:00Z`);
      if (Number.isNaN(date)) throw new Error("window date must be yyyy-mm-dd");
      const holding = movable(await getTokenHoldings(account), p.mint, "NPCS", q);
      // The request PDA is keyed by `queue.requests`: read it now, not at render.
      const raw = await getAccount(address(p.queue));
      if (!raw) throw new Error("the queue is not initialized on this cluster");
      const nextId = codecs.queue.decodeAccount("Queue", raw.data).requests as bigint;
      const ix = await requestRedemptionIx(
        { assetMint: address(p.mint), nextId, holder: account, holderAsset: holding.account, assetTokenProgram: address(p.assetTokenProgram) },
        q,
        BigInt(Math.floor(date / 1000)),
      );
      return send([ix]);
    }, "Redemption requested: it waits for the attester (rank and fill schedule).");
  return (
    <div className="panel">
      <form
        className="row"
        onSubmit={(e) => {
          e.preventDefault();
          void submit();
        }}
      >
        <Field label="NPCS" value={qty} set={setQty} size={10} />
        <Field label="window" value={win} set={setWin} type="date" />
        <button disabled={busy}>request redemption</button>
        <span className="small muted">escrows the NPCS in the request&apos;s own vault until it fills or is cancelled</span>
      </form>
      <TxMsg msg={msg} />
    </div>
  );
}

// ── liquidity-facility ──────────────────────────────────────────────────

export interface FacilityProps {
  /** The wallet the position below was read for. */
  forWallet: string;
  ref_: { config: string; facilityId: string; assetMint: string; usdcMint: string; feed: string; assetTokenProgram: string; usdcTokenProgram: string };
  assetDecimals: number;
  rateBps: string;
  advanceRateBps: string;
  /** Position state name, or null when the wallet has none here. */
  positionState: string | null;
  /** Rails advance: principal, interest, last accrual (null: none / repaid). */
  advance: { principal: string; interest: string; lastAccrual: string } | null;
  /** The wallet's rails payout destination token account, if registered, active and unexpired. */
  payoutToken: string | null;
  payoutProblem: string | null;
}

export function FacilityActions(p: FacilityProps) {
  const { account, send } = useWallet();
  const { busy, msg, run } = useRun();
  const [qty, setQty] = useState("1");
  const [amount, setAmount] = useState("0.1");
  const [topUp, setTopUp] = useState("0.5");
  const [repay, setRepay] = useState("");
  const [now] = useState(() => BigInt(Math.floor(Date.now() / 1000)));
  if (!account) return connect;
  if (account !== p.forWallet) return <p className="small muted">This is {p.forWallet.slice(0, 4)}…{p.forWallet.slice(-4)}&apos;s position: look up the connected wallet to act.</p>;
  const f: FacilityRef = {
    config: address(p.ref_.config),
    facilityId: BigInt(p.ref_.facilityId),
    assetMint: address(p.ref_.assetMint),
    usdcMint: address(p.ref_.usdcMint),
    feed: address(p.ref_.feed),
    assetTokenProgram: address(p.ref_.assetTokenProgram),
    usdcTokenProgram: address(p.ref_.usdcTokenProgram),
  };
  const live = p.positionState === "Active" || p.positionState === "Breached";
  const owed = p.advance
    ? owedAt(BigInt(p.advance.principal), BigInt(p.advance.interest), BigInt(p.advance.lastAccrual), BigInt(p.rateBps), now)
    : 0n;

  const pledge = () =>
    run(async () => {
      if (!p.payoutToken) throw new Error(p.payoutProblem ?? "no payout destination");
      const q = parseUnits(qty, p.assetDecimals);
      const a = parseUnits(amount, 6);
      const h = movable(await getTokenHoldings(account), p.ref_.assetMint, "collateral", q);
      return send([await pledgeAndAdvanceIx({ ...f, borrower: account, borrowerAsset: h.account, destinationToken: address(p.payoutToken) }, q, a)]);
    }, "Pledged and advanced: the USDC is in your payout destination.");
  const add = () =>
    run(async () => {
      const q = parseUnits(topUp, p.assetDecimals);
      const h = movable(await getTokenHoldings(account), p.ref_.assetMint, "collateral", q);
      return send([await addCollateralIx({ ...f, borrower: account, borrowerAsset: h.account }, q)]);
    }, "Collateral added: the next check_health reads it.");
  const pay = (all: boolean) =>
    run(async () => {
      const a = all
        ? owedAt(BigInt(p.advance!.principal), BigInt(p.advance!.interest), BigInt(p.advance!.lastAccrual), BigInt(p.rateBps), BigInt(Math.floor(Date.now() / 1000)))
        : parseUnits(repay, 6);
      if (a <= 0n) throw new Error("nothing to repay");
      const usdc = largest(await getTokenHoldings(account), p.ref_.usdcMint);
      if (!usdc) throw new Error("no USDC account for the facility's mint");
      if (usdc.amount < (all ? BigInt(p.advance!.principal) + BigInt(p.advance!.interest) : a)) throw new Error(`not enough USDC: ${fmt6(usdc.amount)} held`);
      return send([await repayIx({ ...f, borrower: account, payerUsdc: usdc.account }, a)]);
    }, all ? "Repaid in full (the rails take at most what is owed and close the advance)." : "Repayment applied: interest first, then principal.");
  const release = () =>
    run(async () => {
      const hs = await getTokenHoldings(account);
      const { account: dest, pre } = await accountFor(hs, account, f.assetMint, f.assetTokenProgram);
      return send([...pre, await releaseIx({ ...f, borrower: account, borrowerAsset: dest })]);
    }, "Released: the collateral and its lots are back.");

  return (
    <div className="panel">
      {!live && (
        <form
          className="row"
          onSubmit={(e) => {
            e.preventDefault();
            void pledge();
          }}
        >
          <Field label="pledge" value={qty} set={setQty} size={8} />
          <Field label="draw USDC" value={amount} set={setAmount} size={8} />
          <button disabled={busy || !p.payoutToken}>pledge &amp; take advance</button>
          <span className="small muted">
            at most {(Number(p.advanceRateBps) / 100).toFixed(2)}% of the collateral at MARK_PROTECTIVE (Fresh mark only)
          </span>
          {p.payoutProblem && <div className="small bad">{p.payoutProblem}</div>}
        </form>
      )}
      {live && (
        <>
          <form
            className="row"
            onSubmit={(e) => {
              e.preventDefault();
              void add();
            }}
          >
            <Field label="add collateral" value={topUp} set={setTopUp} size={8} />
            <button disabled={busy}>top up</button>
          </form>
          {p.advance ? (
            <form
              className="row"
              onSubmit={(e) => {
                e.preventDefault();
                void pay(false);
              }}
            >
              <Field label="repay USDC" value={repay} set={setRepay} size={8} />
              <button disabled={busy}>repay</button>
              <button type="button" className="ghost" disabled={busy} onClick={() => void pay(true)}>
                repay all (~{fmt6(owed)})
              </button>
            </form>
          ) : (
            <p className="small">
              The advance is repaid.{" "}
              <button disabled={busy} onClick={() => void release()}>
                release collateral
              </button>
            </p>
          )}
        </>
      )}
      <TxMsg msg={msg} />
    </div>
  );
}

// ── tranche-engine ──────────────────────────────────────────────────────

export interface PoolProps {
  pool: string;
  feed: string;
  seniorMint: string;
  juniorMint: string;
  collateralVault: string;
  collateralMint: string;
  collateralDecimals: number;
  collateralTokenProgram: string;
  trancheTokenProgram: string;
  attachmentBps: string;
  open: boolean;
}

export function TrancheActions(p: PoolProps) {
  const { account, send } = useWallet();
  const { busy, msg, run } = useRun();
  const [qty, setQty] = useState("1");
  if (!account) return connect;
  if (!p.open) return <p className="small muted">This pool takes deposits only while Warehousing or Active.</p>;
  const deposit = () =>
    run(async () => {
      const q = parseUnits(qty, p.collateralDecimals);
      const hs = await getTokenHoldings(account);
      const col = movable(hs, p.collateralMint, "collateral", q);
      const tp = address(p.trancheTokenProgram);
      const senior = await accountFor(hs, account, address(p.seniorMint), tp);
      const junior = await accountFor(hs, account, address(p.juniorMint), tp);
      const ix = await trancheDepositIx(
        {
          pool: address(p.pool),
          feed: address(p.feed),
          seniorMint: address(p.seniorMint),
          juniorMint: address(p.juniorMint),
          collateralVault: address(p.collateralVault),
          collateralMint: address(p.collateralMint),
          depositor: account,
          depositorCollateral: col.account,
          depositorSenior: senior.account,
          depositorJunior: junior.account,
          trancheTokenProgram: tp,
          collateralTokenProgram: address(p.collateralTokenProgram),
        },
        q,
      );
      return send([...senior.pre, ...junior.pre, ix]);
    }, "Deposited: senior and junior units minted (junior frozen for the lock-up).");
  return (
    <div className="panel">
      <form
        className="row"
        onSubmit={(e) => {
          e.preventDefault();
          void deposit();
        }}
      >
        <Field label="collateral" value={qty} set={setQty} size={8} />
        <button disabled={busy}>deposit</button>
        <span className="small muted">
          mints junior = value × {(Number(p.attachmentBps) / 100).toFixed(2)}% and senior = the rest, at MARK_PROTECTIVE (Fresh mark only)
        </span>
      </form>
      <TxMsg msg={msg} />
    </div>
  );
}

// ── npcs-treasury ───────────────────────────────────────────────────────

export interface TreasuryProps {
  assetMint: string;
  usdcMint: string;
  vault: string;
  authority: string;
  paused: boolean;
  assetDecimals: number;
  usdcDecimals: number;
  assetTokenProgram: string;
  usdcTokenProgram: string;
}

export function TreasuryActions(p: TreasuryProps) {
  const { account, send } = useWallet();
  const { busy, msg, run } = useRun();
  const [dep, setDep] = useState("1");
  const [red, setRed] = useState("1");
  if (!account) return connect;
  const isAuthority = account === p.authority;
  const accounts = async (needUsdc: bigint, needAsset: bigint) => {
    const hs = await getTokenHoldings(account);
    const usdc = largest(hs, p.usdcMint);
    if (!usdc) throw new Error("no USDC account for the treasury's mint");
    if (usdc.amount < needUsdc) throw new Error(`not enough USDC: ${fmt6(usdc.amount)} held`);
    // NPCS is default-frozen: a fresh account could not receive the mint.
    const asset = movable(hs, p.assetMint, "NPCS", needAsset);
    return {
      assetMint: address(p.assetMint),
      usdcMint: address(p.usdcMint),
      vault: address(p.vault),
      owner: account,
      ownerUsdc: usdc.account,
      ownerAsset: asset.account,
      usdcTokenProgram: address(p.usdcTokenProgram),
      assetTokenProgram: address(p.assetTokenProgram),
    };
  };
  const deposit = () =>
    run(async () => {
      const a = parseUnits(dep, p.usdcDecimals);
      return send([await treasuryDepositIx(await accounts(a, 0n), a)]);
    }, "Deposited: NPCS minted 1:1 (I1 checked before and after).");
  const redeem = () =>
    run(async () => {
      const a = parseUnits(red, p.assetDecimals);
      return send([await treasuryRedeemIx(await accounts(0n, a), a)]);
    }, "Redeemed: NPCS burned, USDC out.");
  return (
    <div className="panel">
      {p.paused ? (
        <p className="small warn">Deposits are paused.</p>
      ) : (
        <form
          className="row"
          onSubmit={(e) => {
            e.preventDefault();
            void deposit();
          }}
        >
          <Field label="USDC" value={dep} set={setDep} size={8} />
          <button disabled={busy}>deposit → NPCS</button>
          <span className="small muted">your NPCS account must be gate-thawed first</span>
        </form>
      )}
      {isAuthority ? (
        <form
          className="row"
          onSubmit={(e) => {
            e.preventDefault();
            void redeem();
          }}
        >
          <Field label="NPCS" value={red} set={setRed} size={8} />
          <button disabled={busy}>redeem → USDC</button>
          <span className="small muted">treasury authority only</span>
        </form>
      ) : (
        <p className="small muted">
          Redeem is the treasury authority&apos;s ({p.authority.slice(0, 4)}…{p.authority.slice(-4)}): holders exit through the redemption
          queue. When the authority is the Squads vault, it is a vault proposal, not a browser signature.
        </p>
      )}
      <TxMsg msg={msg} />
    </div>
  );
}
