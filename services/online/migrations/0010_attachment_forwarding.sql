-- Keep a physical resource group while any independent attachment references it.
ALTER TABLE attachments ADD COLUMN payload_id UUID REFERENCES attachments(id);
ALTER TABLE attachments ADD CHECK (payload_id IS NULL OR (payload_id <> id AND complete));
CREATE INDEX attachments_payload ON attachments(payload_id) WHERE payload_id IS NOT NULL;

-- Preserve only a metadata fingerprint, not the original filename or payload.
CREATE FUNCTION m1_attachment_fingerprint(TEXT, TEXT, BIGINT, TEXT) RETURNS BYTEA
LANGUAGE sql STABLE STRICT AS $$
    SELECT sha256(convert_to(jsonb_build_array($1, $2, $3, $4)::text, 'UTF8'))
$$;
ALTER TABLE messages ADD COLUMN attachment_fingerprint BYTEA;
UPDATE messages m SET attachment_fingerprint = m1_attachment_fingerprint(a.name, a.kind, a.size, a.sha256)
FROM attachments a WHERE m.attachment_id = a.id AND a.complete;

CREATE TABLE m1_forward_batches (
    id UUID PRIMARY KEY,
    owner_id UUID NOT NULL REFERENCES accounts(id),
    source_id UUID NOT NULL REFERENCES messages(attachment_id),
    targets JSONB NOT NULL,
    upload_id UUID UNIQUE
);
ALTER TABLE messages ADD COLUMN forward_batch UUID REFERENCES m1_forward_batches(id);
