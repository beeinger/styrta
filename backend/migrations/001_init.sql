CREATE EXTENSION IF NOT EXISTS vector;
CREATE EXTENSION IF NOT EXISTS pg_trgm;
CREATE EXTENSION IF NOT EXISTS unaccent;

CREATE TABLE innovations (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    source_key text NOT NULL DEFAULT 'rops',
    slug text NOT NULL,
    title text NOT NULL,
    title_key text NOT NULL,
    page_url text NOT NULL,
    card_summary text,
    licence text,
    page_sections jsonb NOT NULL DEFAULT '[]'::jsonb,
    page_sha256 text,
    zip_url text,
    zip_sha256 text,
    zip_bytes bigint,
    zip_etag text,
    zip_last_modified text,
    digest_text text,
    digest_json jsonb,
    agent_version integer,
    status text NOT NULL DEFAULT 'catalogued',
    error text,
    first_seen_at timestamptz NOT NULL DEFAULT now(),
    last_seen_at timestamptz NOT NULL DEFAULT now(),
    digested_at timestamptz,
    CONSTRAINT innovations_source_slug UNIQUE (source_key, slug),
    CONSTRAINT innovations_status CHECK (status IN ('catalogued', 'digested', 'failed'))
);

CREATE INDEX innovations_title_key_idx ON innovations (title_key);
CREATE INDEX innovations_digest_trgm ON innovations USING gin (digest_text gin_trgm_ops);

CREATE TABLE innovation_categories (
    innovation_id uuid NOT NULL REFERENCES innovations (id) ON DELETE CASCADE,
    slug text NOT NULL,
    name text NOT NULL,
    PRIMARY KEY (innovation_id, slug)
);

CREATE TABLE innovation_links (
    id bigserial PRIMARY KEY,
    innovation_id uuid NOT NULL REFERENCES innovations (id) ON DELETE CASCADE,
    kind text NOT NULL,
    url text NOT NULL,
    label text,
    CONSTRAINT innovation_links_unique UNIQUE (innovation_id, url)
);

CREATE TABLE innovation_files (
    id bigserial PRIMARY KEY,
    innovation_id uuid NOT NULL REFERENCES innovations (id) ON DELETE CASCADE,
    path text NOT NULL,
    byte_len bigint NOT NULL,
    sha256 text,
    media_kind text NOT NULL,
    extracted_text text,
    extract_note text,
    CONSTRAINT innovation_files_unique UNIQUE (innovation_id, path)
);

-- Embedding column is filled by a later job. 1024 matches multilingual-e5-large.
CREATE TABLE innovation_chunks (
    id bigserial PRIMARY KEY,
    innovation_id uuid NOT NULL REFERENCES innovations (id) ON DELETE CASCADE,
    ordinal integer NOT NULL,
    field text NOT NULL,
    body text NOT NULL,
    embedding vector(1024),
    embedding_model text,
    CONSTRAINT innovation_chunks_unique UNIQUE (innovation_id, ordinal)
);

CREATE TABLE ingest_runs (
    id bigserial PRIMARY KEY,
    kind text NOT NULL,
    started_at timestamptz NOT NULL DEFAULT now(),
    finished_at timestamptz,
    stats jsonb NOT NULL DEFAULT '{}'::jsonb
);
