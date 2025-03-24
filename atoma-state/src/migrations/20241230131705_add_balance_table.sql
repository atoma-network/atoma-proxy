CREATE TABLE balance (
  -- Unique identifier for the user
  user_id INTEGER PRIMARY KEY,

  -- USDC balance for the user
  usdc_balance BIGINT NOT NULL CHECK (usdc_balance > 0),

  -- Last timestamp for the user
  usdc_last_timestamp BIGINT NOT NULL
);

CREATE UNIQUE INDEX idx_unique_sui_address ON users(sui_address) WHERE sui_address IS NOT NULL;
