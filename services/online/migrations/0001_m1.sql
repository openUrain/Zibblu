CREATE TABLE accounts (
    id UUID PRIMARY KEY,
    handle TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL
);

CREATE TABLE sessions (
    token_hash BYTEA PRIMARY KEY,
    account_id UUID NOT NULL REFERENCES accounts (id),
    expires_at TIMESTAMPTZ NOT NULL DEFAULT now() + interval '7 days'
);
CREATE INDEX sessions_account ON sessions (account_id);

CREATE TABLE contacts (
    owner_id UUID NOT NULL REFERENCES accounts (id),
    contact_id UUID NOT NULL REFERENCES accounts (id),
    PRIMARY KEY (owner_id, contact_id),
    CHECK (owner_id <> contact_id)
);

CREATE TABLE conversations (
    id UUID PRIMARY KEY,
    title TEXT,
    direct_key TEXT UNIQUE,
    created_at BIGINT NOT NULL DEFAULT (extract(epoch FROM clock_timestamp()) * 1000)::BIGINT,
    CHECK ((title IS NULL) <> (direct_key IS NULL))
);

CREATE TABLE members (
    conversation_id UUID NOT NULL REFERENCES conversations (id),
    account_id UUID NOT NULL REFERENCES accounts (id),
    PRIMARY KEY (conversation_id, account_id)
);
CREATE INDEX members_account ON members (account_id, conversation_id);

CREATE TABLE attachments (
    id UUID PRIMARY KEY,
    conversation_id UUID NOT NULL,
    owner_id UUID NOT NULL,
    name TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('file', 'folder')),
    size BIGINT NOT NULL CHECK (size >= 0),
    sha256 TEXT NOT NULL,
    received BIGINT NOT NULL DEFAULT 0 CHECK (received BETWEEN 0 AND size),
    complete BOOLEAN NOT NULL DEFAULT FALSE,
    FOREIGN KEY (conversation_id, owner_id) REFERENCES members (conversation_id, account_id)
);
CREATE INDEX attachments_owner ON attachments (owner_id);

CREATE TABLE attachment_chunks (
    attachment_id UUID NOT NULL REFERENCES attachments (id) ON DELETE CASCADE,
    byte_offset BIGINT NOT NULL CHECK (byte_offset >= 0),
    data BYTEA NOT NULL,
    PRIMARY KEY (attachment_id, byte_offset)
);

CREATE TABLE messages (
    seq BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    nonce UUID,
    conversation_id UUID NOT NULL,
    sender_id UUID NOT NULL,
    created_at BIGINT NOT NULL DEFAULT (extract(epoch FROM clock_timestamp()) * 1000)::BIGINT,
    text TEXT,
    attachment_id UUID UNIQUE REFERENCES attachments (id),
    UNIQUE (sender_id, nonce),
    FOREIGN KEY (conversation_id, sender_id) REFERENCES members (conversation_id, account_id),
    CHECK ((text IS NULL) <> (attachment_id IS NULL)),
    CHECK ((text IS NULL) = (nonce IS NULL))
);
CREATE INDEX messages_conversation ON messages (conversation_id, seq DESC);
