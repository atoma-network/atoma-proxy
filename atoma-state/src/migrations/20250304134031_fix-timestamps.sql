-- Rename column creation_timestamp to old_creation_timestamp in users table
ALTER TABLE users RENAME COLUMN creation_timestamp TO old_creation_timestamp;

-- Add new column creation_timestamp with default value CURRENT_TIMESTAMP in users table
ALTER TABLE users ADD COLUMN creation_timestamp TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP;

-- Update the new column creation_timestamp with the value of old_creation_timestamp in users table
UPDATE users SET creation_timestamp = old_creation_timestamp;

-- Drop column old_creation_timestamp in users table
ALTER TABLE users DROP COLUMN old_creation_timestamp;

-- Rename column creation_timestamp to old_creation_timestamp in api_tokens table
ALTER TABLE api_tokens RENAME COLUMN creation_timestamp TO old_creation_timestamp;

-- Add new column creation_timestamp with default value CURRENT_TIMESTAMP in api_tokens table
ALTER TABLE api_tokens ADD COLUMN creation_timestamp TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP;

-- Update the new column creation_timestamp with the value of old_creation_timestamp in api_tokens table
UPDATE api_tokens SET creation_timestamp = old_creation_timestamp;

-- Drop column old_creation_timestamp in api_tokens table
ALTER TABLE api_tokens DROP COLUMN old_creation_timestamp;

-- Add creation_timestamp column to nodes table
ALTER TABLE nodes ADD COLUMN creation_timestamp TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP;
