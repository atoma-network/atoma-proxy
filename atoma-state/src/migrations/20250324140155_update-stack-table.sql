-- Add new columns to the stacks table for the claimed stack event
ALTER TABLE stacks 
    ADD COLUMN is_claimed BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN user_refund_amount INTEGER;
