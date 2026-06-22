-- USDC mode (ADR-0010/0011): positions can settle off-chain in points (default,
-- unchanged) or on-chain in USDC against the tick_vault. USDC positions carry
-- the on-chain object ids captured at mint so the settler knows what to settle
-- and which PlayerBalance to credit; settlements carry the Walrus proof state.

ALTER TABLE positions
  ADD COLUMN settle_mode       TEXT NOT NULL DEFAULT 'points',
  ADD COLUMN vault_position_id TEXT,
  ADD COLUMN vault_id          TEXT,
  ADD COLUMN player_balance_id TEXT,
  ADD COLUMN owner_address     TEXT,
  ADD CONSTRAINT positions_settle_mode_chk CHECK (settle_mode IN ('points', 'usdc')),
  -- A USDC position is meaningless without its on-chain anchors; a points
  -- position must not carry them. Enforced so the dual-sink branch can trust
  -- the row shape.
  ADD CONSTRAINT positions_usdc_ids_present CHECK (
    settle_mode = 'points'
    OR (vault_position_id IS NOT NULL
        AND vault_id IS NOT NULL
        AND player_balance_id IS NOT NULL
        AND owner_address IS NOT NULL)
  );

CREATE INDEX positions_open_usdc ON positions (status, t_close_ms)
  WHERE status = 'OPEN' AND settle_mode = 'usdc';

ALTER TABLE settlements
  ADD COLUMN proof_status    TEXT NOT NULL DEFAULT 'pending',
  ADD COLUMN sui_tx_digest   TEXT,
  ADD COLUMN walrus_blob_id  TEXT,
  -- Upper bound of the evidence tick span ([oracle_seq_at_tap .. evidence_to_seq]):
  -- the touch tick for a win, the expiry tick for a loss. Persisted so the proof
  -- retry sweep can reassemble the evidence window without the live tick.
  ADD COLUMN evidence_to_seq BIGINT,
  ADD CONSTRAINT settlements_proof_status_chk
    CHECK (proof_status IN ('pending', 'published', 'failed'));

-- Drives the proof-publish retry sweep: find USDC settlements whose Walrus proof
-- hasn't been published yet. Points settlements never need a proof, but they
-- default to 'pending' too, so the sweep filters by the position's settle_mode.
CREATE INDEX settlements_proof_unpublished ON settlements (proof_status)
  WHERE proof_status IN ('pending', 'failed');
