CREATE TABLE lan_authority (
    singleton BOOLEAN PRIMARY KEY CHECK (singleton),
    certificate BYTEA NOT NULL,
    signing_key BYTEA NOT NULL
);
