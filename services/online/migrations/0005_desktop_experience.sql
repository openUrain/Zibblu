ALTER TABLE accounts
    ADD COLUMN bio TEXT NOT NULL DEFAULT '',
    ADD COLUMN read_receipts BOOLEAN NOT NULL DEFAULT FALSE;

ALTER TABLE contacts
    ADD COLUMN remark TEXT NOT NULL DEFAULT '',
    ADD COLUMN contact_info TEXT NOT NULL DEFAULT '';

ALTER TABLE contact_requests
    ADD COLUMN request_id UUID NOT NULL DEFAULT gen_random_uuid(),
    ADD COLUMN greeting TEXT NOT NULL DEFAULT '';

ALTER TABLE members ADD COLUMN read_through BIGINT NOT NULL DEFAULT 0;
UPDATE members own SET read_through = coalesce((
    SELECT max(seq) FROM messages WHERE conversation_id = own.conversation_id
), 0);

ALTER TABLE messages ADD COLUMN reply_to BIGINT REFERENCES messages (seq);

CREATE TABLE message_receipts (
    message_seq BIGINT NOT NULL REFERENCES messages (seq) ON DELETE CASCADE,
    account_id UUID NOT NULL REFERENCES accounts (id),
    PRIMARY KEY (message_seq, account_id)
);
CREATE INDEX message_receipts_reader ON message_receipts (account_id);

CREATE TABLE profile_images (
    account_id UUID NOT NULL REFERENCES accounts (id),
    kind TEXT NOT NULL CHECK (kind IN ('avatar', 'cover')),
    version UUID NOT NULL,
    data BYTEA NOT NULL CHECK (octet_length(data) BETWEEN 1 AND 2097152),
    PRIMARY KEY (account_id, kind)
);
