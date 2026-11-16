-- This release initializes a fresh M1 database. Never reinterpret legacy history as a queue.
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM accounts) THEN
        RAISE EXCEPTION 'Local-First requires a new Zibblu database. Keep the existing database for rollback.';
    END IF;
END
$$;

ALTER TABLE accounts ADD COLUMN revoked_at TIMESTAMPTZ;
ALTER TABLE members ADD COLUMN active BOOLEAN NOT NULL DEFAULT true;
ALTER TABLE members DROP COLUMN delivered_through, DROP COLUMN read_through;
DROP TABLE pending_delivery_batches, message_receipts;

ALTER TABLE messages
    DROP CONSTRAINT messages_check,
    DROP CONSTRAINT messages_check1,
    DROP CONSTRAINT messages_attachment_id_fkey,
    ADD COLUMN published_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    ADD COLUMN expires_at TIMESTAMPTZ,
    ADD CHECK ((attachment_id IS NULL) <> (nonce IS NULL)),
    ADD CHECK (attachment_id IS NULL OR (text IS NULL AND ai_provider IS NULL));
CREATE INDEX messages_expiring ON messages (expires_at, seq) WHERE text IS NOT NULL;

ALTER TABLE attachments ADD COLUMN expires_at TIMESTAMPTZ;
CREATE INDEX uploads_expiring ON attachments (expires_at, id);
CREATE TABLE m1_expired_uploads (
    id UUID PRIMARY KEY,
    owner_id UUID NOT NULL,
    conversation_id UUID NOT NULL,
    forget_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX expired_uploads_forget ON m1_expired_uploads (forget_at);
ALTER TABLE contact_requests
    ADD COLUMN greeting_expires_at TIMESTAMPTZ,
    ADD UNIQUE (request_id);
CREATE INDEX greetings_expiring ON contact_requests (greeting_expires_at) WHERE greeting <> '';
CREATE TABLE m1_contact_keys (
    request_id UUID PRIMARY KEY,
    requester_id UUID NOT NULL REFERENCES accounts (id),
    recipient_id UUID NOT NULL REFERENCES accounts (id)
);

CREATE TABLE m1_feed (
    singleton BOOLEAN PRIMARY KEY CHECK (singleton),
    epoch UUID NOT NULL,
    committed_through BIGINT NOT NULL
);
INSERT INTO m1_feed VALUES (true, gen_random_uuid(), 0);

CREATE TABLE m1_devices (
    id UUID PRIMARY KEY,
    account_id UUID NOT NULL REFERENCES accounts (id),
    credential_hash BYTEA NOT NULL,
    epoch UUID NOT NULL,
    last_seen_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    active_until TIMESTAMPTZ,
    retired_at TIMESTAMPTZ,
    acked_after BIGINT NOT NULL,
    cursor UUID NOT NULL,
    page_id UUID,
    page_next BIGINT,
    page_offered BIGINT[],
    page_more BOOLEAN,
    UNIQUE (account_id, credential_hash),
    UNIQUE (account_id, id)
);
CREATE INDEX m1_devices_expiring ON m1_devices (active_until) WHERE retired_at IS NULL;
ALTER TABLE sessions
    ADD COLUMN device_id UUID NOT NULL,
    ADD FOREIGN KEY (account_id, device_id) REFERENCES m1_devices (account_id, id);
CREATE INDEX sessions_device ON sessions (device_id);

CREATE TABLE m1_recipients (
    message_seq BIGINT NOT NULL REFERENCES messages (seq),
    account_id UUID NOT NULL REFERENCES accounts (id),
    metadata_received BOOLEAN NOT NULL DEFAULT false,
    content_verified BOOLEAN NOT NULL DEFAULT false,
    PRIMARY KEY (message_seq, account_id)
);
CREATE INDEX m1_recipients_account ON m1_recipients (account_id, message_seq);

CREATE VIEW m1_visible_messages AS
SELECT r.account_id, r.metadata_received, r.content_verified, m.*
FROM m1_recipients r JOIN messages m ON m.seq = r.message_seq
JOIN members own ON own.account_id = r.account_id AND own.conversation_id = m.conversation_id
JOIN accounts a ON a.id = r.account_id
WHERE own.active AND m.seq > own.joined_after AND a.revoked_at IS NULL;

CREATE TABLE m1_device_events (
    device_id UUID NOT NULL REFERENCES m1_devices (id),
    event_seq BIGINT NOT NULL,
    message_seq BIGINT NOT NULL REFERENCES messages (seq),
    metadata_state TEXT NOT NULL DEFAULT 'pending' CHECK (metadata_state IN ('pending', 'received', 'processed')),
    content_state TEXT CHECK (content_state IN ('pending', 'verified', 'expired', 'revoked')),
    PRIMARY KEY (device_id, event_seq)
);
CREATE INDEX m1_events_message ON m1_device_events (message_seq, device_id);
CREATE INDEX m1_events_pending ON m1_device_events (device_id, event_seq) WHERE metadata_state = 'pending';
