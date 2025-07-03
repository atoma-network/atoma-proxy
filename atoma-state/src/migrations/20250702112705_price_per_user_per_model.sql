-- Add migration script here
CREATE TABLE
  user_model_prices (
    user_id UUID NOT NULL,
    model_name TEXT NOT NULL,
    price_per_one_million_input_compute_units DECIMAL(10, 4) NOT NULL,
    price_per_one_million_output_compute_units DECIMAL(10, 4) NOT NULL,
    creation_timestamp TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (user_id, model_name, creation_timestamp)
  );
