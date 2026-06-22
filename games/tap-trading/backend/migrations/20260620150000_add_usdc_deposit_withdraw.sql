-- USDC deposit/withdraw rails (in-app funding for the USDC economy).
--
-- Custody model: one operator-owned vault PlayerBalance holds all players'
-- deposited USDC. A player deposits by signing a `vault::deposit` tx into that
-- custody balance; the API verifies the on-chain tx and credits the off-chain
-- USDC ledger (`accounts.balance`, micro-units). Withdraw debits the ledger and
-- the operator signs a release tx back to the player's bound wallet address.

-- Bind an account to the Sui wallet address it deposits from / withdraws to.
ALTER TABLE accounts ADD COLUMN IF NOT EXISTS sui_address TEXT;

-- On-chain deposits, idempotent by tx digest (one credit per tx, ever).
CREATE TABLE IF NOT EXISTS deposits (
  id            BIGSERIAL PRIMARY KEY,
  account_id    BIGINT NOT NULL REFERENCES accounts(id),
  tx_digest     TEXT   NOT NULL UNIQUE,
  amount_micro  BIGINT NOT NULL CHECK (amount_micro > 0),
  from_address  TEXT   NOT NULL,
  created_at_ms BIGINT NOT NULL
);

-- Withdrawals: ledger debited up front; the operator release tx digest is
-- recorded once it lands (status PENDING -> SENT, or FAILED on release error).
CREATE TABLE IF NOT EXISTS withdrawals (
  id            BIGSERIAL PRIMARY KEY,
  account_id    BIGINT NOT NULL REFERENCES accounts(id),
  amount_micro  BIGINT NOT NULL CHECK (amount_micro > 0),
  to_address    TEXT   NOT NULL,
  tx_digest     TEXT,
  status        TEXT   NOT NULL DEFAULT 'PENDING',
  created_at_ms BIGINT NOT NULL
);
CREATE INDEX IF NOT EXISTS withdrawals_account ON withdrawals (account_id);
