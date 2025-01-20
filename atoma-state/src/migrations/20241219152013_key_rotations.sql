-- Add migration script here
CREATE TABLE IF NOT EXISTS key_rotations (
    epoch BIGINT PRIMARY KEY,
    key_rotation_counter BIGINT NOT NULL
);
