CREATE TABLE
  node_attestations (
    node_small_id BIGINT PRIMARY KEY NOT NULL,
    attestation BYTEA NOT NULL,
    updated_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP
  );
