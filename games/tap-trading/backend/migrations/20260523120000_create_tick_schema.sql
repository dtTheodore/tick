-- Tick: initial schema. SYSTEM_DESIGN §2.
-- One migration per logical change; this is the genesis migration for the Tick
-- backend foundation. Tables: accounts, points_ledger, streaks, positions,
-- settlements, daily_quests, snapshots, flags.

-- §2.1
CREATE TABLE accounts (
  id                  BIGSERIAL PRIMARY KEY,
  external_id         TEXT NOT NULL UNIQUE,
  zklogin_sub         TEXT NOT NULL,
  zklogin_iss         TEXT NOT NULL,
  display_name        VARCHAR(64),
  tier                SMALLINT NOT NULL DEFAULT 1,
  balance             BIGINT NOT NULL DEFAULT 0,
  lifetime_points_won BIGINT NOT NULL DEFAULT 0,
  flag_state          VARCHAR(16) NOT NULL DEFAULT 'OK',
  signup_bonus_at_ms  BIGINT,
  created_at_ms       BIGINT NOT NULL,
  last_active_ms      BIGINT NOT NULL,
  CHECK (balance >= 0),
  CHECK (tier >= 1),
  CHECK (flag_state IN ('OK', 'SOFT_FLAG', 'HARD_FLAG'))
);

CREATE INDEX accounts_external_id ON accounts (external_id);
CREATE INDEX accounts_lifetime_points ON accounts (lifetime_points_won DESC);
CREATE INDEX accounts_last_active ON accounts (last_active_ms DESC);

-- §2.4
CREATE TABLE points_ledger (
  id            BIGSERIAL PRIMARY KEY,
  account_id    BIGINT NOT NULL REFERENCES accounts(id),
  kind          VARCHAR(24) NOT NULL,
  delta         BIGINT NOT NULL,
  ref_id        BIGINT,
  created_at_ms BIGINT NOT NULL
);

CREATE INDEX ledger_account ON points_ledger (account_id, created_at_ms DESC);
CREATE INDEX ledger_kind ON points_ledger (kind, created_at_ms DESC);

-- §2.5
CREATE TABLE streaks (
  account_id     BIGINT PRIMARY KEY REFERENCES accounts(id),
  current_streak INT NOT NULL DEFAULT 0,
  max_streak     INT NOT NULL DEFAULT 0,
  updated_at_ms  BIGINT NOT NULL,
  CHECK (current_streak >= 0),
  CHECK (max_streak >= current_streak)
);

-- §2.2
CREATE TABLE positions (
  id                  BIGSERIAL PRIMARY KEY,
  account_id          BIGINT NOT NULL REFERENCES accounts(id),
  asset               VARCHAR(16) NOT NULL,
  strike_lo           NUMERIC(20, 8) NOT NULL,
  strike_hi           NUMERIC(20, 8) NOT NULL,
  t_open_ms           BIGINT NOT NULL,
  t_close_ms          BIGINT NOT NULL,
  stake_points        BIGINT NOT NULL,
  multiplier_at_tap   NUMERIC(10, 4) NOT NULL,
  status              VARCHAR(16) NOT NULL DEFAULT 'OPEN',
  settled_at_ms       BIGINT,
  client_fingerprint   TEXT,
  ip_hash              BYTEA,
  created_at_ms        BIGINT NOT NULL,
  oracle_seq_at_tap    BIGINT NOT NULL,
  oracle_run_id_at_tap BIGINT NOT NULL,
  client_request_id    UUID NOT NULL,
  CHECK (asset IN ('ETH', 'BTC', 'SOL')),
  CHECK (oracle_seq_at_tap >= 0),
  CONSTRAINT positions_dedup_request UNIQUE (account_id, client_request_id),
  CHECK (strike_hi > strike_lo),
  CHECK (strike_lo > 0),
  CHECK (t_close_ms > t_open_ms),
  CHECK (stake_points > 0),
  CHECK (multiplier_at_tap > 0),
  CHECK (status IN ('OPEN', 'WON', 'LOST', 'VOIDED'))
);

CREATE INDEX positions_account ON positions (account_id, created_at_ms DESC);
CREATE INDEX positions_open ON positions (status, t_close_ms) WHERE status = 'OPEN';
CREATE INDEX positions_settle_window ON positions (asset, t_open_ms, t_close_ms);

-- §2.3
CREATE TABLE settlements (
  id                  BIGSERIAL PRIMARY KEY,
  position_id         BIGINT NOT NULL UNIQUE REFERENCES positions(id),
  account_id          BIGINT NOT NULL,
  outcome             CHAR(1) NOT NULL,
  points_delta        BIGINT NOT NULL,
  oracle_price        NUMERIC(20, 8) NOT NULL,
  settled_at_ms       BIGINT NOT NULL,
  multiplier_used     NUMERIC(10, 4) NOT NULL,
  streak_at_credit    INT NOT NULL,
  streak_bonus        NUMERIC(5, 3) NOT NULL,
  CHECK (outcome IN ('W', 'L', 'V')),
  CHECK (oracle_price > 0),
  CHECK (multiplier_used > 0),
  CHECK (streak_at_credit >= 0)
);

CREATE INDEX settlements_account ON settlements (account_id, settled_at_ms DESC);

-- §2.6
CREATE TABLE daily_quests (
  id              BIGSERIAL PRIMARY KEY,
  account_id      BIGINT NOT NULL REFERENCES accounts(id),
  quest_code      VARCHAR(32) NOT NULL,
  utc_date        DATE NOT NULL,
  progress        INT NOT NULL DEFAULT 0,
  target          INT NOT NULL,
  reward_points   INT NOT NULL,
  completed_at_ms BIGINT,
  UNIQUE (account_id, quest_code, utc_date),
  CHECK (progress >= 0),
  CHECK (target > 0),
  CHECK (reward_points >= 0)
);

CREATE INDEX quests_account_date ON daily_quests (account_id, utc_date);

-- §2.7 (reads land in Plan B verify endpoint; writes in Phase 3 anchor-publisher)
CREATE TABLE snapshots (
  week_idx        BIGINT PRIMARY KEY,
  merkle_root     BYTEA NOT NULL,
  total_users     BIGINT NOT NULL,
  total_points    NUMERIC(30, 0) NOT NULL,
  on_chain_tx     TEXT NOT NULL,
  published_at_ms BIGINT NOT NULL,
  CHECK (total_users >= 0),
  CHECK (total_points >= 0)
);

-- §2.8
CREATE TABLE flags (
  id             BIGSERIAL PRIMARY KEY,
  account_id     BIGINT NOT NULL REFERENCES accounts(id),
  flag_code      VARCHAR(32) NOT NULL,
  severity       VARCHAR(8) NOT NULL,
  evidence       JSONB NOT NULL,
  reviewed_at_ms BIGINT,
  resolution     VARCHAR(16),
  created_at_ms  BIGINT NOT NULL,
  CHECK (severity IN ('SOFT', 'HARD'))
);

CREATE INDEX flags_account ON flags (account_id, created_at_ms DESC);
CREATE INDEX flags_open ON flags (severity) WHERE reviewed_at_ms IS NULL;
