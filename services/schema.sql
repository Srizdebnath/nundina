-- nundina indexer schema (SRS 08 §6: sessions, bids, fills, lots, marks,
-- claims, advances, waterfall lines). Postgres; also runs on PGlite.
-- Idempotent: applied on every start. Amounts are NUMERIC (u64/u128 exact).
--
-- `events` is the source of truth: every decoded program event, keyed by
-- (signature, event_index). The other tables are projections rebuilt from
-- it; every projection is a SET (never an increment), so re-ingesting a
-- transaction (backfill overlapping a webhook) changes nothing.

CREATE TABLE IF NOT EXISTS events (
  signature    TEXT    NOT NULL,
  event_index  INTEGER NOT NULL,
  slot         BIGINT  NOT NULL,
  block_time   BIGINT,
  program      TEXT    NOT NULL,
  name         TEXT    NOT NULL,
  data         JSONB   NOT NULL,
  PRIMARY KEY (signature, event_index)
);
CREATE INDEX IF NOT EXISTS events_program_name ON events (program, name, slot);
CREATE INDEX IF NOT EXISTS events_slot ON events (slot);

-- Per-source high-water mark for the RPC backfill.
CREATE TABLE IF NOT EXISTS cursors (
  source        TEXT PRIMARY KEY,
  last_signature TEXT,
  last_slot     BIGINT,
  updated_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- auction-engine
CREATE TABLE IF NOT EXISTS sessions (
  session        TEXT PRIMARY KEY,
  phase          TEXT,
  phase_slot     BIGINT,
  clear_price    NUMERIC,
  cleared_volume NUMERIC,
  reserve        NUMERIC,
  reference_mark NUMERIC,
  failed_reason  TEXT,
  asset_mint     TEXT,
  print_price    NUMERIC,
  print_volume   NUMERIC,
  n_investor_ids INTEGER,
  forced_sale    BOOLEAN
);
CREATE TABLE IF NOT EXISTS bids (
  session     TEXT NOT NULL,
  owner       TEXT NOT NULL,
  side        TEXT NOT NULL,
  commitment  TEXT,
  escrowed    NUMERIC,
  revealed    BOOLEAN NOT NULL DEFAULT false,
  limit_price NUMERIC,
  qty         NUMERIC,
  PRIMARY KEY (session, owner)
);
CREATE TABLE IF NOT EXISTS fills (
  session      TEXT    NOT NULL,
  fill_index   INTEGER NOT NULL,
  buyer        TEXT    NOT NULL,
  seller       TEXT    NOT NULL,
  qty          NUMERIC NOT NULL,
  price        NUMERIC,
  gross        NUMERIC,
  to_seller    NUMERIC,
  fee          NUMERIC,
  to_reserve   NUMERIC,
  rofr         BOOLEAN NOT NULL DEFAULT false,
  settled      BOOLEAN NOT NULL DEFAULT false,
  settle_signature TEXT,
  PRIMARY KEY (session, fill_index)
);

-- mark-engine
CREATE TABLE IF NOT EXISTS marks (
  feed          TEXT    NOT NULL,
  computed_ts   BIGINT  NOT NULL,
  nav_pico      NUMERIC NOT NULL,
  effective_ts  BIGINT  NOT NULL,
  model_version INTEGER NOT NULL,
  inputs_hash   TEXT    NOT NULL,
  admin_drill   BOOLEAN NOT NULL,
  signature     TEXT    NOT NULL,
  PRIMARY KEY (feed, computed_ts)
);
CREATE TABLE IF NOT EXISTS mark_states (
  feed      TEXT   NOT NULL,
  at        BIGINT NOT NULL,
  from_state TEXT  NOT NULL,
  to_state  TEXT   NOT NULL,
  breaker   TEXT,
  PRIMARY KEY (feed, at, to_state)
);
CREATE TABLE IF NOT EXISTS prints (
  feed           TEXT NOT NULL,
  session        TEXT NOT NULL,
  price          NUMERIC,
  volume         NUMERIC,
  n_investor_ids INTEGER,
  forced_sale    BOOLEAN,
  eligibility    TEXT,
  implied_pico   NUMERIC,
  signature      TEXT NOT NULL,
  PRIMARY KEY (feed, session)
);

-- credit-gate: the lot ledger is evented, not snapshotted (append / split /
-- merge-to-free from LotEvent, and each settle's seller debit 'sale' from
-- SettleChecked); a holder's ledger is the fold.
CREATE TABLE IF NOT EXISTS lot_events (
  signature   TEXT    NOT NULL,
  event_index INTEGER NOT NULL,
  slot        BIGINT  NOT NULL,
  mint        TEXT    NOT NULL,
  owner       TEXT    NOT NULL,
  kind        TEXT    NOT NULL,
  lot_id      INTEGER,
  qty         NUMERIC NOT NULL,
  restriction_expiry BIGINT,
  free_qty    NUMERIC,
  PRIMARY KEY (signature, event_index, owner)
);
CREATE INDEX IF NOT EXISTS lot_events_owner ON lot_events (mint, owner, slot);
CREATE TABLE IF NOT EXISTS gate_decisions (
  signature   TEXT    NOT NULL,
  event_index INTEGER NOT NULL,
  slot        BIGINT  NOT NULL,
  mint        TEXT    NOT NULL,
  owner       TEXT    NOT NULL,
  reject      TEXT,
  distinct_investors INTEGER,
  PRIMARY KEY (signature, event_index)
);

-- queue-claim
CREATE TABLE IF NOT EXISTS claims (
  request       TEXT PRIMARY KEY,
  holder        TEXT,
  qty           NUMERIC,
  window_date   BIGINT,
  rank          INTEGER,
  claim_mint    TEXT,
  state         TEXT,
  state_slot    BIGINT,
  filled_qty    NUMERIC,
  transferable  BOOLEAN,
  pledgeable    BOOLEAN,
  pledged_facility TEXT,
  last_pv       NUMERIC
);

-- liquidity-facility
CREATE TABLE IF NOT EXISTS advances (
  position   TEXT PRIMARY KEY,
  borrower   TEXT,
  qty        NUMERIC,
  collateral_value NUMERIC,
  advanced   NUMERIC,
  state      TEXT,
  state_slot BIGINT,
  last_ltv_bps NUMERIC,
  shortfall  NUMERIC,
  released   BOOLEAN NOT NULL DEFAULT false
);

-- tranche-engine waterfall, one row per paid period, and every rails payout.
CREATE TABLE IF NOT EXISTS waterfall_lines (
  pool          TEXT    NOT NULL,
  period        INTEGER NOT NULL,
  cash          NUMERIC NOT NULL,
  fees          NUMERIC NOT NULL,
  senior_coupon NUMERIC NOT NULL,
  reserve       NUMERIC NOT NULL,
  senior_amort  NUMERIC NOT NULL,
  junior_coupon NUMERIC NOT NULL,
  junior_residual NUMERIC NOT NULL,
  unpaid_senior_coupon NUMERIC NOT NULL,
  signature     TEXT    NOT NULL,
  PRIMARY KEY (pool, period)
);
CREATE TABLE IF NOT EXISTS payouts (
  signature   TEXT    NOT NULL,
  event_index INTEGER NOT NULL,
  kind        TEXT    NOT NULL,
  source      TEXT    NOT NULL,
  destination TEXT    NOT NULL,
  amount      NUMERIC NOT NULL,
  PRIMARY KEY (signature, event_index)
);
