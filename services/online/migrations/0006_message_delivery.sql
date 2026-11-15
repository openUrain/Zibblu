ALTER TABLE members
    ADD COLUMN delivered_through BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN viewed_through BIGINT NOT NULL DEFAULT 0;

ALTER TABLE accounts ADD COLUMN receipts_after BIGINT NOT NULL DEFAULT 0;

CREATE TABLE pending_delivery_batches (
    session_hash BYTEA NOT NULL,
    served_after BIGINT NOT NULL,
    id UUID NOT NULL UNIQUE,
    recipient_id UUID NOT NULL REFERENCES accounts (id),
    sequences BIGINT[] NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL DEFAULT now() + interval '7 days',
    PRIMARY KEY (session_hash, served_after)
);
CREATE INDEX pending_delivery_batches_expiry ON pending_delivery_batches (expires_at);
