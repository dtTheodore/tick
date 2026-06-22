-- Replace the third supported quote SOL with SUI on `positions.asset`.
--
-- The original CHECK allowed ('ETH','BTC','SOL'); the SOL price feed was
-- replaced by SUI end-to-end (oracle aggregator, pricing engine, API), so the
-- constraint must follow or the DB silently rejects every SUI insert.
--
-- Done as a forward migration (not an edit to the create-schema file) so the
-- already-applied original keeps its checksum — sqlx's migrate-or-fail boot
-- guard rejects a modified applied migration. No existing row uses 'SOL' (the
-- client only ever wrote 'ETH'), so the re-validation is a no-op on live data.
--
-- The original CHECK is an inline, Postgres-auto-named constraint, so drop it by
-- definition (its generated name isn't guaranteed) and re-add a named one.
DO $$
DECLARE
  cname text;
BEGIN
  SELECT conname INTO cname
  FROM pg_constraint
  WHERE conrelid = 'positions'::regclass
    AND contype = 'c'
    AND pg_get_constraintdef(oid) ILIKE '%asset%';
  IF cname IS NOT NULL THEN
    EXECUTE format('ALTER TABLE positions DROP CONSTRAINT %I', cname);
  END IF;
END $$;

ALTER TABLE positions
  ADD CONSTRAINT positions_asset_check CHECK (asset IN ('ETH', 'BTC', 'SUI'));
