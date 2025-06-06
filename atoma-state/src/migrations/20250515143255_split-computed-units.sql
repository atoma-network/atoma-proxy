BEGIN;

ALTER TABLE fiat_balance
RENAME COLUMN already_debited_amount TO already_debited_completions_amount;

ALTER TABLE fiat_balance
RENAME COLUMN overcharged_unsettled_amount TO overcharged_unsettled_completions_amount;

ALTER TABLE fiat_balance
ADD COLUMN already_debited_input_amount BIGINT NOT NULL DEFAULT 0,
ADD COLUMN overcharged_unsettled_input_amount BIGINT NOT NULL DEFAULT 0;

ALTER TABLE usage_per_model
RENAME COLUMN total_number_processed_tokens TO total_input_tokens;

ALTER TABLE usage_per_model
ADD COLUMN total_output_tokens BIGINT NOT NULL DEFAULT 0;

ALTER TABLE IF EXISTS fiat_balance
RENAME TO fiat_balances;

COMMIT;
