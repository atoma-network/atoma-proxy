-- This migration adds a name column to the api_tokens table.
-- This column will be used to store the last 4 characters of the token for the existing tokens.
ALTER TABLE api_tokens
ADD COLUMN name TEXT DEFAULT 'temporary default';

UPDATE api_tokens
SET name = RIGHT(token, 4);

ALTER TABLE api_tokens
ALTER COLUMN name SET NOT NULL,
ALTER COLUMN name DROP DEFAULT;
