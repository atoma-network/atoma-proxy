CREATE TABLE
  IF NOT EXISTS usage_per_day (
    user_id BIGINT NOT NULL,
    timestamp TIMESTAMP NOT NULL,
    model TEXT NOT NULL,
    input_amount BIGINT NOT NULL DEFAULT 0,
    input_tokens BIGINT NOT NULL DEFAULT 0,
    output_amount BIGINT NOT NULL DEFAULT 0,
    output_tokens BIGINT NOT NULL DEFAULT 0,
    UNIQUE (user_id, timestamp, model)
  );
