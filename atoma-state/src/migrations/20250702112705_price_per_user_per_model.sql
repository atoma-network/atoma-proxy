-- Add migration script here
CREATE TABLE
  user_model_prices (
    user_id BIGINT NOT NULL,
    model_name TEXT NOT NULL,
    price_per_one_million_input_compute_units BIGINT NOT NULL,
    price_per_one_million_output_compute_units BIGINT NOT NULL,
    creation_timestamp TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (user_id, model_name, creation_timestamp)
  );
