-- node registration table
CREATE TABLE IF NOT EXISTS nodes (
    node_small_id          BIGINT  PRIMARY KEY,
    node_id                TEXT    NOT NULL,
    sui_address            TEXT    NOT NULL,
    public_address         TEXT,
    timestamp              BIGINT
);
