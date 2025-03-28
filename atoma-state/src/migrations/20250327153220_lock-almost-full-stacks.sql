-- Add lock state to the stack table
ALTER TABLE stacks ADD COLUMN is_locked BOOLEAN NOT NULL DEFAULT FALSE;
