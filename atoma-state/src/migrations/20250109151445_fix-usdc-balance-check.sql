-- Drop the existing check constraint
ALTER TABLE balance
DROP CONSTRAINT balance_usdc_balance_check;

-- Add the new check constraint
ALTER TABLE balance
ADD CONSTRAINT balance_usdc_balance_check CHECK (usdc_balance >= 0);
