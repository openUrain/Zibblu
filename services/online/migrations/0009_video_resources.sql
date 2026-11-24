-- Derivatives are separate resources, not attachments or new messages.
CREATE TABLE video_resources (
    attachment_id UUID NOT NULL REFERENCES attachments(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('cover', 'preview')),
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'ready', 'unsupported', 'failed', 'not_smaller', 'quota')),
    size BIGINT,
    sha256 TEXT,
    PRIMARY KEY (attachment_id, kind),
    CHECK (
        (status = 'ready' AND size IS NOT NULL AND size > 0
         AND size <= CASE kind WHEN 'cover' THEN 524288 ELSE 33554432 END
         AND sha256 IS NOT NULL AND sha256 ~ '^[0-9a-f]{64}$')
        OR (status <> 'ready' AND size IS NULL AND sha256 IS NULL)
    )
);
CREATE INDEX video_resources_pending ON video_resources (attachment_id) WHERE kind = 'cover' AND status = 'pending';
CREATE TABLE video_resource_chunks (
    attachment_id UUID NOT NULL,
    kind TEXT NOT NULL,
    byte_offset BIGINT NOT NULL CHECK (byte_offset >= 0 AND byte_offset % 1048576 = 0),
    data BYTEA NOT NULL CHECK (octet_length(data) BETWEEN 1 AND 1048576),
    PRIMARY KEY (attachment_id, kind, byte_offset),
    FOREIGN KEY (attachment_id, kind) REFERENCES video_resources(attachment_id, kind) ON DELETE CASCADE
);
