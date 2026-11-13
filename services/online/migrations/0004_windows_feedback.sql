CREATE UNIQUE INDEX accounts_handle_lower ON accounts ((lower(handle COLLATE "C")));

CREATE TABLE contact_requests (
    requester_id UUID NOT NULL REFERENCES accounts (id),
    recipient_id UUID NOT NULL REFERENCES accounts (id),
    PRIMARY KEY (requester_id, recipient_id),
    CHECK (requester_id <> recipient_id)
);
CREATE UNIQUE INDEX contact_requests_pair
    ON contact_requests (least(requester_id, recipient_id), greatest(requester_id, recipient_id));
CREATE INDEX contact_requests_recipient ON contact_requests (recipient_id);

-- An existing reciprocal contact is already approved by both accounts.
INSERT INTO contact_requests (requester_id, recipient_id)
SELECT owner_id, contact_id FROM contacts c
WHERE NOT EXISTS (
    SELECT 1 FROM contacts r WHERE r.owner_id = c.contact_id AND r.contact_id = c.owner_id
);
DELETE FROM contacts c USING contact_requests r
WHERE c.owner_id = r.requester_id AND c.contact_id = r.recipient_id;

ALTER TABLE messages
    ADD COLUMN recall_seq BIGINT,
    ADD COLUMN ai_provider TEXT CHECK (ai_provider IN ('open_ai', 'deep_seek', 'grok'));
CREATE UNIQUE INDEX messages_recall_seq ON messages (recall_seq) WHERE recall_seq IS NOT NULL;
CREATE INDEX messages_conversation_time ON messages (conversation_id, created_at DESC, seq DESC);

CREATE TABLE ai_reply_readers (
    message_seq BIGINT NOT NULL REFERENCES messages (seq) ON DELETE CASCADE,
    account_id UUID NOT NULL REFERENCES accounts (id),
    PRIMARY KEY (message_seq, account_id)
);
