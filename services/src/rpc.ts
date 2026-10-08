/** Solana RPC with retries on network errors (public RPCs drop connections under load). */
import {
  createDefaultRpcTransport,
  createSolanaRpcFromTransport,
  createSolanaRpcSubscriptions,
  type Rpc,
  type RpcTransport,
  type SolanaRpcApi,
} from "@solana/kit";

export type R = Rpc<SolanaRpcApi>;

export function makeRpc(url: string): R {
  const base = createDefaultRpcTransport({ url });
  const retrying: RpcTransport = async <T>(req: Parameters<RpcTransport>[0]): Promise<T> => {
    for (let attempt = 0; ; attempt++) {
      try {
        return await base<T>(req);
      } catch (e) {
        const network = e instanceof TypeError || /fetch failed|ECONNRESET|ETIMEDOUT|socket|429/i.test(String((e as Error)?.message));
        if (!network || attempt >= 4) throw e;
        await new Promise((r) => setTimeout(r, 500 * 2 ** attempt));
      }
    }
  };
  return createSolanaRpcFromTransport(retrying);
}

/** ws URL for an http RPC URL: same host, scheme ws(s), port + 1 if explicit (the crank's rule). */
export function wsFor(rpcUrl: string): string {
  const u = new URL(rpcUrl);
  u.protocol = u.protocol === "https:" ? "wss:" : "ws:";
  if (u.port) u.port = String(Number(u.port) + 1);
  return u.toString();
}

export const subscriptions = (url: string) => createSolanaRpcSubscriptions(wsFor(url));

export const RPC_URL = process.env.RPC_URL ?? "http://127.0.0.1:8899";
