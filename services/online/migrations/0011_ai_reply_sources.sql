CREATE TABLE ai_reply_sources (
    reply_seq BIGINT NOT NULL REFERENCES messages(seq) ON DELETE CASCADE,
    number INTEGER NOT NULL CHECK (number BETWEEN 1 AND 64),
    source_seq BIGINT NOT NULL REFERENCES messages(seq) ON DELETE CASCADE,
    attachment_id UUID,
    PRIMARY KEY (reply_seq, number)
);

CREATE INDEX ai_reply_sources_source ON ai_reply_sources (source_seq);
