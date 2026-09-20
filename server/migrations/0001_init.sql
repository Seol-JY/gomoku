-- Domain model: users/credentials = anonymous accounts, moves = current game only, events = append-only stream log
-- No board state: `moves` is the current move list, `events` the append-only stream log.
-- `identities` stays empty until web login exists.

CREATE TABLE users (
    id           UUID PRIMARY KEY,
    display_name TEXT NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL
);

CREATE TABLE identities (
    user_id     UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    provider    TEXT NOT NULL,
    external_id TEXT NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (provider, external_id)
);
CREATE INDEX idx_identities_user ON identities(user_id);

CREATE TABLE credentials (
    token_hash   TEXT PRIMARY KEY,
    user_id      UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    device       TEXT,
    created_at   TIMESTAMPTZ NOT NULL,
    expires_at   TIMESTAMPTZ,
    last_used_at TIMESTAMPTZ
);
CREATE INDEX idx_credentials_user ON credentials(user_id);

CREATE TABLE rooms (
    id                UUID PRIMARY KEY,
    invite_code       TEXT NOT NULL UNIQUE,
    invite_expires_at TIMESTAMPTZ NOT NULL,
    visibility        TEXT NOT NULL DEFAULT 'private'
                      CHECK (visibility IN ('private', 'public')),
    status            TEXT NOT NULL
                      CHECK (status IN ('waiting', 'playing', 'finished')),
    state_seq         BIGINT NOT NULL DEFAULT 0,
    created_by        UUID NOT NULL REFERENCES users(id),
    created_at        TIMESTAMPTZ NOT NULL,
    finished_at       TIMESTAMPTZ,
    winner            TEXT CHECK (winner IN ('black', 'white')),
    end_reason        TEXT CHECK (end_reason IN ('five', 'resign', 'draw', 'timeout')),
    games_played      INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE memberships (
    room_id   UUID NOT NULL REFERENCES rooms(id) ON DELETE CASCADE,
    user_id   UUID NOT NULL REFERENCES users(id),
    role      TEXT NOT NULL CHECK (role IN ('black', 'white', 'spectator')),
    joined_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (room_id, user_id)
);
CREATE INDEX idx_memberships_user ON memberships(user_id);

CREATE TABLE moves (
    room_id   UUID NOT NULL REFERENCES rooms(id) ON DELETE CASCADE,
    seq       INTEGER NOT NULL,
    user_id   UUID NOT NULL,
    x         SMALLINT NOT NULL CHECK (x BETWEEN 0 AND 14),
    y         SMALLINT NOT NULL CHECK (y BETWEEN 0 AND 14),
    played_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (room_id, seq)
);

CREATE TABLE events (
    room_id    UUID NOT NULL REFERENCES rooms(id) ON DELETE CASCADE,
    seq        BIGINT NOT NULL,
    kind       TEXT NOT NULL,
    payload    JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (room_id, seq)
);
