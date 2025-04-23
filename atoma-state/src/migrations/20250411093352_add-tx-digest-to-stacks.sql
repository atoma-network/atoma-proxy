-- Add tx_digest column to stacks table
ALTER TABLE stacks ADD COLUMN tx_digest TEXT NOT NULL DEFAULT '';

-- Add comment to document that tx_digest is base58 encoded
COMMENT ON COLUMN stacks.tx_digest IS 'Base58 encoded transaction digest';