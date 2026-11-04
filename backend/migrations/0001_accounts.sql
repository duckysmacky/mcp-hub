CREATE TABLE users (
    id                BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    username          TEXT        NOT NULL,
    password_hash     TEXT        NOT NULL,          -- Argon2id PHC string (verification only)
    kek_salt          BYTEA       NOT NULL,          -- salt for the password KEK (not the PHC salt)
    kek_params        JSONB       NOT NULL,          -- {"m":65536,"t":3,"p":1}
    wrapped_dek       BYTEA       NOT NULL,          -- AEAD(KEK_password, DEK)
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX users_username_lower ON users (lower(username));

CREATE TABLE web_sessions (
    id                BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    token_hash        BYTEA       NOT NULL UNIQUE,   -- sha256(cookie token)
    user_id           BIGINT      NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    wrapped_dek       BYTEA       NOT NULL,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at        TIMESTAMPTZ NOT NULL
);
