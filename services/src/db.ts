/**
 * The indexer's database. Production is Postgres (Supabase, SRS 08 §9)
 * through `pg`; `pglite:<dir>` (or `pglite:memory`) runs the same schema on
 * PGlite, real Postgres compiled to WASM, for local runs and the tests.
 */
import { mkdirSync, readFileSync } from "node:fs";

export interface Db {
  query<T = Record<string, unknown>>(sql: string, params?: unknown[]): Promise<T[]>;
  close(): Promise<void>;
}

const SCHEMA = readFileSync(new URL("../schema.sql", import.meta.url), "utf8");

export async function openDb(url: string): Promise<Db> {
  let db: Db;
  if (url.startsWith("pglite:")) {
    const { PGlite } = await import("@electric-sql/pglite");
    const where = url.slice("pglite:".length);
    const memory = where === "memory" || where === "";
    if (!memory) mkdirSync(where, { recursive: true });
    const pg = memory ? new PGlite() : new PGlite(where);
    db = {
      query: async <T>(sql: string, params?: unknown[]) => (await pg.query<T>(sql, params)).rows,
      close: () => pg.close(),
    };
    await pg.exec(SCHEMA);
  } else {
    const { default: pg } = await import("pg");
    // NUMERIC as string (exact), BIGINT as string; callers convert.
    const pool = new pg.Pool({ connectionString: url });
    db = {
      query: async <T>(sql: string, params?: unknown[]) => (await pool.query(sql, params)).rows as T[],
      close: () => pool.end(),
    };
    await pool.query(SCHEMA);
  }
  return db;
}
