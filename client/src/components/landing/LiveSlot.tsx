"use client";

import { useEffect, useState } from "react";
import { DEVNET_RPC_URL } from "@/lib/site";

type EpochInfo = { epoch: number; absoluteSlot: number };

const POLL_MS = 15_000;
const fmt = (n: number) => n.toLocaleString("en-US");

/** Reads the live devnet epoch/slot straight from the cluster RPC. */
export function LiveSlot() {
  const [info, setInfo] = useState<EpochInfo | null>(null);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    let cancelled = false;

    async function poll() {
      try {
        const res = await fetch(DEVNET_RPC_URL, {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "getEpochInfo" }),
        });
        const json = await res.json();
        if (!cancelled && json.result) {
          setInfo({ epoch: json.result.epoch, absoluteSlot: json.result.absoluteSlot });
          setFailed(false);
        }
      } catch {
        if (!cancelled) setFailed(true);
      }
    }

    poll();
    const id = setInterval(poll, POLL_MS);
    return () => {
      cancelled = true;
      clearInterval(id);
    };
  }, []);

  if (!info) {
    return <span>Solana devnet • {failed ? "RPC unreachable" : "Reading slot…"}</span>;
  }
  return (
    <span>
      Solana devnet • Epoch {fmt(info.epoch)} • Slot #{fmt(info.absoluteSlot)}
    </span>
  );
}
