CREATE TABLE invites (
    id                BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    code_hash         BYTEA       NOT NULL UNIQUE,   -- sha256(invite code)
    note              TEXT,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at        TIMESTAMPTZ NOT NULL,
    used_at           TIMESTAMPTZ,
    used_by           BIGINT      REFERENCES users(id) ON DELETE SET NULL
);
