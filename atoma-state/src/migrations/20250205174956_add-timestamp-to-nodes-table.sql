-- add timestamp and node_id to nodes table
ALTER TABLE nodes
    ADD COLUMN timestamp BIGINT,
    ADD COLUMN node_id TEXT NOT NULL;
