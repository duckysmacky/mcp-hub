CREATE TABLE oauth_tokens (
    id                BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    token_hash        BYTEA       NOT NULL UNIQUE,
    kind              TEXT        NOT NULL CHECK (kind IN ('access', 'refresh')),
    grant_id          BIGINT      NOT NULL REFERENCES oauth_grants(id) ON DELETE CASCADE,
    issued_from_code  BIGINT      REFERENCES oauth_codes(id) ON DELETE SET NULL,   -- code-replay revocation
    rotated_from      BIGINT      REFERENCES oauth_tokens(id) ON DELETE SET NULL,
    wrapped_dek       BYTEA       NOT NULL,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at        TIMESTAMPTZ NOT NULL,
    used_at           TIMESTAMPTZ,                   -- refresh: set on rotation (reuse = attack)
    revoked_at        TIMESTAMPTZ
);
CREATE INDEX tokens_grant_idx ON oauth_tokens (grant_id);
