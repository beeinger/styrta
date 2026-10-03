CREATE TABLE users (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    display_name text NOT NULL,
    locale text NOT NULL DEFAULT 'pl'
);

CREATE TABLE profiles (
    user_id uuid PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    age_band text,
    gender text,
    mobility text,
    sportiness smallint,
    bio text,
    women_only boolean NOT NULL DEFAULT false,
    start_minute smallint,
    end_minute smallint,
    embedding vector(1024),
    embedding_model text,
    CONSTRAINT profiles_sportiness_range CHECK (
        sportiness IS NULL OR (sportiness >= 0 AND sportiness <= 3)
    ),
    CONSTRAINT profiles_time_window_pair CHECK (
        (start_minute IS NULL AND end_minute IS NULL)
        OR (start_minute IS NOT NULL AND end_minute IS NOT NULL)
    ),
    CONSTRAINT profiles_time_window_range CHECK (
        (start_minute IS NULL OR (start_minute >= 0 AND start_minute <= 1440))
        AND (end_minute IS NULL OR (end_minute >= 0 AND end_minute <= 1440))
    )
);

CREATE TABLE profile_tags (
    user_id uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    tag text NOT NULL,
    polarity text NOT NULL,
    PRIMARY KEY (user_id, tag),
    CONSTRAINT profile_tags_polarity CHECK (polarity IN ('like', 'dislike'))
);

CREATE TABLE memories (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    durability text NOT NULL,
    key text NOT NULL,
    value text NOT NULL,
    quote text,
    confidence real,
    confirmed boolean NOT NULL DEFAULT false,
    CONSTRAINT memories_durability CHECK (durability IN ('long_term', 'short_term'))
);

CREATE INDEX memories_user_idx ON memories (user_id);
CREATE INDEX memories_value_trgm ON memories USING gin (value gin_trgm_ops);
CREATE INDEX memories_quote_trgm ON memories USING gin (quote gin_trgm_ops);

CREATE TABLE places (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name text NOT NULL,
    kind text NOT NULL,
    lat double precision NOT NULL,
    lon double precision NOT NULL,
    CONSTRAINT places_kind CHECK (kind IN ('cafe', 'park', 'hall', 'square', 'other_public'))
);

CREATE TABLE events (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    host_id uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    place_id uuid NOT NULL REFERENCES places (id),
    title text NOT NULL,
    emoji text NOT NULL,
    description text,
    starts_at timestamptz NOT NULL,
    capacity integer,
    activity_tags text[] NOT NULL DEFAULT '{}',
    promoted boolean NOT NULL DEFAULT false,
    status text NOT NULL DEFAULT 'scheduled',
    women_only boolean NOT NULL DEFAULT false,
    CONSTRAINT events_status CHECK (status IN ('scheduled', 'cancelled')),
    CONSTRAINT events_capacity_nonnegative CHECK (capacity IS NULL OR capacity >= 0)
);

CREATE INDEX events_starts_at_idx ON events (starts_at);
CREATE INDEX events_host_idx ON events (host_id);

CREATE TABLE attendances (
    event_id uuid NOT NULL REFERENCES events (id) ON DELETE CASCADE,
    user_id uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    status text NOT NULL,
    PRIMARY KEY (event_id, user_id),
    CONSTRAINT attendances_status CHECK (status IN ('going', 'cancelled', 'completed'))
);

CREATE INDEX attendances_user_status_idx ON attendances (user_id, status);
CREATE INDEX attendances_event_status_idx ON attendances (event_id, status);

CREATE TABLE messages (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    role text NOT NULL,
    body text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    CONSTRAINT messages_role CHECK (role IN ('user', 'assistant'))
);

CREATE INDEX messages_user_created_idx ON messages (user_id, created_at, id);
CREATE INDEX messages_body_trgm ON messages USING gin (body gin_trgm_ops);

CREATE TABLE conversations (
    user_id uuid PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    summary text,
    summary_through uuid REFERENCES messages (id)
);

CREATE TABLE turns (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    status text NOT NULL DEFAULT 'running',
    user_text text,
    reply_text text,
    audio_path text,
    checkpoint jsonb,
    error text,
    CONSTRAINT turns_status CHECK (status IN ('running', 'done', 'failed'))
);

CREATE INDEX turns_user_idx ON turns (user_id);
CREATE INDEX turns_running_idx ON turns (id) WHERE status = 'running';

CREATE TABLE stream_events (
    id bigserial PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    turn_id uuid REFERENCES turns (id) ON DELETE CASCADE,
    kind text NOT NULL,
    payload jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp()
);

CREATE INDEX stream_events_user_id_idx ON stream_events (user_id, id);
