/**
 * api (SRS 08 §6): read-only JSON over the indexer's tables. No write
 * routes: every user action is a wallet-signed transaction built
 * client-side (the UI does this, `app/lib/tx.ts`); the server never
 * custodies or signs (ME-4).
 *
 *   GET /health                      cursors + row counts
 *   GET /sessions                    every indexed auction session
 *   GET /sessions/:session           session + bids + fills + print + its events
 *   GET /holders/:wallet             lot events, claims, advances, bids, gate decisions
 *   GET /marks/:feed                 NAV publications + mark-state changes + prints
 *   GET /pools/:pool/waterfall       one line per distributed period
 *   GET /events?program=&name=&limit=   raw decoded events, newest first
 *
 * Amounts are decimal strings (u64/u128 exact).
 */
import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";
import type { Db } from "./db.ts";

type Handler = (db: Db, params: string[], query: URLSearchParams) => Promise<unknown>;

const ADDRESS = "([1-9A-HJ-NP-Za-km-z]{32,44})";

const routes: [RegExp, Handler][] = [
  [
    /^\/health$/,
    async (db) => ({
      ok: true,
      cursors: await db.query(`SELECT source, last_signature, last_slot, updated_at FROM cursors ORDER BY source`),
      events: (await db.query<{ n: string }>(`SELECT count(*)::text AS n FROM events`))[0]?.n,
    }),
  ],
  [/^\/sessions$/, async (db) => db.query(`SELECT * FROM sessions ORDER BY phase_slot DESC NULLS LAST`)],
  [
    new RegExp(`^/sessions/${ADDRESS}$`),
    async (db, [s]) => {
      const [session] = await db.query(`SELECT * FROM sessions WHERE session = $1`, [s]);
      if (!session) return null;
      return {
        session,
        bids: await db.query(`SELECT * FROM bids WHERE session = $1 ORDER BY owner`, [s]),
        fills: await db.query(`SELECT * FROM fills WHERE session = $1 ORDER BY fill_index`, [s]),
        print: (await db.query(`SELECT * FROM prints WHERE session = $1`, [s]))[0] ?? null,
        events: await db.query(
          `SELECT signature, event_index, slot, block_time, program, name, data FROM events
           WHERE data->>'session' = $1 ORDER BY slot, signature, event_index`,
          [s],
        ),
      };
    },
  ],
  [
    new RegExp(`^/holders/${ADDRESS}$`),
    async (db, [w]) => ({
      lot_events: await db.query(`SELECT * FROM lot_events WHERE owner = $1 ORDER BY slot, event_index`, [w]),
      claims: await db.query(`SELECT * FROM claims WHERE holder = $1 ORDER BY request`, [w]),
      advances: await db.query(`SELECT * FROM advances WHERE borrower = $1 ORDER BY position`, [w]),
      bids: await db.query(`SELECT * FROM bids WHERE owner = $1 ORDER BY session`, [w]),
      gate_decisions: await db.query(`SELECT * FROM gate_decisions WHERE owner = $1 ORDER BY slot`, [w]),
    }),
  ],
  [
    new RegExp(`^/marks/${ADDRESS}$`),
    async (db, [f]) => ({
      publications: await db.query(`SELECT * FROM marks WHERE feed = $1 ORDER BY computed_ts`, [f]),
      states: await db.query(`SELECT * FROM mark_states WHERE feed = $1 ORDER BY at`, [f]),
      prints: await db.query(`SELECT * FROM prints WHERE feed = $1 ORDER BY session`, [f]),
    }),
  ],
  [new RegExp(`^/pools/${ADDRESS}/waterfall$`), async (db, [p]) => db.query(`SELECT * FROM waterfall_lines WHERE pool = $1 ORDER BY period`, [p])],
  [
    /^\/events$/,
    async (db, _p, q) => {
      const limit = Math.min(Math.max(Number(q.get("limit") ?? 100) || 100, 1), 1000);
      return db.query(
        `SELECT signature, event_index, slot, block_time, program, name, data FROM events
         WHERE ($1::text IS NULL OR program = $1) AND ($2::text IS NULL OR name = $2)
         ORDER BY slot DESC, signature, event_index LIMIT $3`,
        [q.get("program"), q.get("name"), limit],
      );
    },
  ],
];

export async function handle(db: Db, req: IncomingMessage, res: ServerResponse): Promise<void> {
  const url = new URL(req.url ?? "/", "http://x");
  const json = (code: number, body: unknown): void =>
    void res
      .writeHead(code, { "content-type": "application/json", "access-control-allow-origin": "*" })
      .end(JSON.stringify(body, (_k, v) => (typeof v === "bigint" ? v.toString() : v)));
  if (req.method !== "GET") return json(405, { error: "read-only API" });
  for (const [re, h] of routes) {
    const m = re.exec(url.pathname);
    if (!m) continue;
    try {
      const body = await h(db, m.slice(1), url.searchParams);
      return body === null ? json(404, { error: "not found" }) : json(200, body);
    } catch (e) {
      return json(500, { error: (e as Error).message });
    }
  }
  return json(404, { error: "no such route" });
}

export function apiServer(db: Db): Server {
  return createServer((req, res) => void handle(db, req, res));
}
