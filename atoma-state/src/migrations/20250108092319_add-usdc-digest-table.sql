CREATE TABLE usdc_payment_digests (
  -- Unique identifier for the payment digest
  digest TEXT PRIMARY KEY
);

ALTER TABLE balance
DROP COLUMN usdc_last_timestamp;
