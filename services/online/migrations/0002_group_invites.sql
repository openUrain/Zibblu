ALTER TABLE members ADD COLUMN joined_after BIGINT NOT NULL DEFAULT 0;

CREATE TABLE group_invites (
    conversation_id UUID PRIMARY KEY REFERENCES conversations (id) ON DELETE CASCADE,
    code_hash BYTEA NOT NULL UNIQUE,
    expires_at TIMESTAMPTZ NOT NULL
);
