CREATE TABLE
  IF NOT EXISTS fiat_balance (
    user_id INTEGER NOT NULL PRIMARY KEY,
    usd_balance BIGINT NOT NULL DEFAULT 0,
    already_paid BIGINT NOT NULL DEFAULT 0,
    locked BIGINT NOT NULL DEFAULT 0,
    num_requests BIGINT NOT NULL DEFAULT 0
  );

CREATE TABLE
  IF NOT EXISTS usage_per_model (
    user_id INTEGER NOT NULL,
    model STRING NOT NULL,
    total_number_processed_tokens_per_model BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (user_id, model)
  );
