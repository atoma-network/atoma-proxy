-- Add migration script here
CREATE TABLE IF NOT EXISTS key_rotations (
    -- Unique identifier for the key rotation
    epoch BIGINT PRIMARY KEY,

    -- Key rotation counter for the key rotation
    key_rotation_counter BIGINT NOT NULL
);
