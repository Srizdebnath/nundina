"use client";
/**
 * Wallet Standard connection (SRS 08 §7: Phantom, Backpack, Squads — real
 * wallets only, no embedded or keypairless mode). The wallet signs; the page
 * sends through the configured RPC and waits for confirmation.
 *
 * Phantom is the primary wallet: it is listed first and offered as the
 * install / mobile deep link when no wallet is detected. Every other wallet
 * that registers through the Wallet Standard (Solflare, Backpack, Glow,
 * Squads, …) is listed after it. The last wallet used reconnects silently on
 * the next visit.
 */
import React, { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from "react";
import { getWallets } from "@wallet-standard/app";
import type { Wallet, WalletAccount } from "@wallet-standard/base";
import {
  appendTransactionMessageInstructions,
  compileTransaction,
  createTransactionMessage,
  getBase64Decoder,
  getTransactionEncoder,
  pipe,
  setTransactionMessageFeePayer,
  setTransactionMessageLifetimeUsingBlockhash,
  address,
  type Address,
  type Base64EncodedWireTransaction,
  type Instruction,
} from "@solana/kit";
import { rpc } from "./rpc";

type ConnectFeature = { connect: (input?: { silent?: boolean }) => Promise<{ accounts: readonly WalletAccount[] }> };
type EventsFeature = {
  on: (event: "change", listener: (props: { accounts?: readonly WalletAccount[] }) => void) => () => void;
};
type DisconnectFeature = { disconnect: () => Promise<void> };
type SignTxFeature = {
  signTransaction: (
    ...inputs: { account: WalletAccount; transaction: Uint8Array; chain?: string }[]
  ) => Promise<readonly { signedTransaction: Uint8Array }[]>;
};

type SignMsgFeature = {
  signMessage: (...inputs: { account: WalletAccount; message: Uint8Array }[]) => Promise<readonly { signature: Uint8Array }[]>;
};

const SIGN = "solana:signTransaction";
const SIGN_MESSAGE = "solana:signMessage";

const LAST_WALLET = "nundina.wallet";
export const PRIMARY_WALLET = "Phantom";

/** Solana-capable wallets, Phantom first, then alphabetical. */
function solanaWallets(): Wallet[] {
  return getWallets()
    .get()
    .filter((w) => w.chains.some((c) => c.startsWith("solana:")) && SIGN in w.features && "standard:connect" in w.features)
    .sort((a, b) => Number(b.name === PRIMARY_WALLET) - Number(a.name === PRIMARY_WALLET) || a.name.localeCompare(b.name));
}

function remember(name: string | null) {
  try {
    if (name) localStorage.setItem(LAST_WALLET, name);
    else localStorage.removeItem(LAST_WALLET);
  } catch {
    // storage blocked (private mode): reconnect just won't be automatic
  }
}

function lastWallet(): string | null {
  try {
    return localStorage.getItem(LAST_WALLET);
  } catch {
    return null;
  }
}

const solanaAccount = (accounts: readonly WalletAccount[]) =>
  accounts.find((x) => x.chains.some((c) => c.startsWith("solana:"))) ?? accounts[0] ?? null;

interface Ctx {
  wallets: Wallet[];
  wallet: Wallet | null;
  account: Address | null;
  connect: (w: Wallet) => Promise<void>;
  disconnect: () => Promise<void>;
  /** Sign with the connected wallet, send, wait for `confirmed`. Returns the signature. */
  send: (ixs: Instruction[]) => Promise<string>;
  /** Sign an off-chain message (the allowlist challenge). */
  signMessage: (message: Uint8Array) => Promise<Uint8Array>;
}

const WalletCtx = createContext<Ctx | null>(null);

export function WalletProvider({ children }: { children: React.ReactNode }) {
  const [wallets, setWallets] = useState<Wallet[]>([]);
  const [wallet, setWallet] = useState<Wallet | null>(null);
  const [acct, setAcct] = useState<WalletAccount | null>(null);

  useEffect(() => {
    const api = getWallets();
    const refresh = () => setWallets(solanaWallets());
    refresh();
    const offs = [api.on("register", refresh), api.on("unregister", refresh)];
    return () => offs.forEach((off) => off());
  }, []);

  const connect = useCallback(async (w: Wallet) => {
    const { accounts } = await (w.features["standard:connect"] as ConnectFeature).connect();
    const a = solanaAccount(accounts);
    if (!a) throw new Error(`${w.name} returned no account`);
    setWallet(w);
    setAcct(a);
    remember(w.name);
  }, []);

  // Silent reconnect to the wallet used last time (no popup if it is still authorized).
  const tried = useRef(false);
  useEffect(() => {
    if (tried.current || wallet) return;
    const name = lastWallet();
    const w = name ? wallets.find((x) => x.name === name) : undefined;
    if (!w) return;
    tried.current = true;
    (w.features["standard:connect"] as ConnectFeature)
      .connect({ silent: true })
      .then(({ accounts }) => {
        const a = solanaAccount(accounts);
        if (a) {
          setWallet(w);
          setAcct(a);
        }
      })
      .catch(() => undefined);
  }, [wallets, wallet]);

  // Follow account switches inside the wallet; an empty list means it disconnected.
  useEffect(() => {
    const ev = wallet?.features["standard:events"] as EventsFeature | undefined;
    if (!ev) return;
    return ev.on("change", ({ accounts }) => {
      if (!accounts) return;
      const a = solanaAccount(accounts);
      if (a) setAcct(a);
      else {
        setWallet(null);
        setAcct(null);
        remember(null);
      }
    });
  }, [wallet]);

  const disconnect = useCallback(async () => {
    const f = wallet?.features["standard:disconnect"] as DisconnectFeature | undefined;
    await f?.disconnect().catch(() => undefined);
    setWallet(null);
    setAcct(null);
    remember(null);
  }, [wallet]);

  const send = useCallback(
    async (ixs: Instruction[]) => {
      if (!wallet || !acct) throw new Error("connect a wallet first");
      const payer = address(acct.address);
      const { value: bh } = await rpc.getLatestBlockhash({ commitment: "confirmed" }).send();
      const msg = pipe(
        createTransactionMessage({ version: 0 }),
        (m) => setTransactionMessageFeePayer(payer, m),
        (m) => setTransactionMessageLifetimeUsingBlockhash(bh, m),
        (m) => appendTransactionMessageInstructions(ixs, m),
      );
      const bytes = new Uint8Array(getTransactionEncoder().encode(compileTransaction(msg)));
      const [signed] = await (wallet.features[SIGN] as SignTxFeature).signTransaction({
        account: acct,
        transaction: bytes,
        chain: acct.chains.find((c) => c.startsWith("solana:")),
      });
      if (!signed) throw new Error("wallet returned no transaction");
      const wire = getBase64Decoder().decode(signed.signedTransaction) as Base64EncodedWireTransaction;
      const sig = await rpc.sendTransaction(wire, { encoding: "base64", preflightCommitment: "confirmed" }).send();
      for (let i = 0; i < 60; i++) {
        const { value } = await rpc.getSignatureStatuses([sig]).send();
        const st = value[0];
        if (st?.err) throw new Error(`transaction failed: ${JSON.stringify(st.err, (_k, v) => (typeof v === "bigint" ? v.toString() : v))}`);
        if (st && (st.confirmationStatus === "confirmed" || st.confirmationStatus === "finalized")) return sig;
        await new Promise((r) => setTimeout(r, 1000));
      }
      throw new Error(`not confirmed after 60s: ${sig}`);
    },
    [wallet, acct],
  );

  const signMessage = useCallback(
    async (message: Uint8Array) => {
      if (!wallet || !acct) throw new Error("connect a wallet first");
      const f = wallet.features[SIGN_MESSAGE] as SignMsgFeature | undefined;
      if (!f) throw new Error(`${wallet.name} cannot sign messages`);
      const [out] = await f.signMessage({ account: acct, message });
      if (!out) throw new Error("wallet returned no signature");
      return out.signature;
    },
    [wallet, acct],
  );

  const value = useMemo<Ctx>(
    () => ({ wallets, wallet, account: acct ? address(acct.address) : null, connect, disconnect, send, signMessage }),
    [wallets, wallet, acct, connect, disconnect, send, signMessage],
  );
  return <WalletCtx.Provider value={value}>{children}</WalletCtx.Provider>;
}

export function useWallet(): Ctx {
  const c = useContext(WalletCtx);
  if (!c) throw new Error("useWallet outside WalletProvider");
  return c;
}
