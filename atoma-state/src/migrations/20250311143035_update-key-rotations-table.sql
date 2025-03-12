
-- Add nonce column to the key_rotations table
ALTER TABLE key_rotations ADD COLUMN nonce BIGINT NOT NULL;

-- Add device_type and task_small_id columns to the node_public_keys table
ALTER TABLE node_public_keys ADD COLUMN device_type INTEGER NOT NULL;
ALTER TABLE node_public_keys ADD COLUMN task_small_id BIGINT NOT NULL;

-- Rename tee_remote_attestation_bytes to remote_attestation_bytes in the node_public_keys table
ALTER TABLE node_public_keys RENAME COLUMN tee_remote_attestation_bytes TO evidence_bytes;

-- Drop the existing primary key constraint
ALTER TABLE node_public_keys DROP CONSTRAINT IF EXISTS node_public_keys_pkey;

-- Add a new composite primary key
ALTER TABLE node_public_keys ADD PRIMARY KEY (node_small_id, device_type);

-- Add a new column to the key_rotations table
ALTER TABLE key_rotations ADD COLUMN nonce BIGINT NOT NULL;