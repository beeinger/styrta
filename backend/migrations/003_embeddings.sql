ALTER TABLE events
    ADD COLUMN embedding vector(1024),
    ADD COLUMN embedding_model text;

CREATE INDEX memories_key_trgm ON memories USING gin (key gin_trgm_ops);
