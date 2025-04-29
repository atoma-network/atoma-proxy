CREATE TABLE
  IF NOT EXISTS fiat_balance (
    user_id BIGINT NOT NULL PRIMARY KEY,
    usd_balance BIGINT NOT NULL DEFAULT 0,
    already_debited_amount BIGINT NOT NULL DEFAULT 0,
    overcharged_unsettled_amount BIGINT NOT NULL DEFAULT 0,
    num_requests BIGINT NOT NULL DEFAULT 0
  );

CREATE TABLE
  IF NOT EXISTS usage_per_model (
    user_id BIGINT NOT NULL,
    model TEXT NOT NULL,
    total_number_processed_tokens BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (user_id, model)
  );

  ALTER TABLE IF EXISTS balance RENAME TO crypto_balances;
