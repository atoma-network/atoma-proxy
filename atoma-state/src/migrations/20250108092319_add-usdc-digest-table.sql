CREATE TABLE usdc_payment_digests (
  digest TEXT PRIMARY KEY
);

ALTER TABLE balance
DROP COLUMN usdc_last_timestamp;
