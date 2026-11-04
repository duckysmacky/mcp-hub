CREATE TABLE oauth_clients (
    id                BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    client_id         TEXT        NOT NULL UNIQUE,
    client_name       TEXT,                          -- self-reported
    redirect_uris     JSONB       NOT NULL,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE oauth_pending_authorizations (
    id                TEXT        PRIMARY KEY,       -- random, appears in /consent?request=
    client_id         TEXT        NOT NULL REFERENCES oauth_clients(client_id) ON DELETE CASCADE,
    redirect_uri      TEXT        NOT NULL,
    code_challenge    TEXT        NOT NULL,
    state             TEXT,
    scope             TEXT        NOT NULL,
    resource          TEXT        NOT NULL,          -- canonical <public_url>/mcp/<slug>
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at        TIMESTAMPTZ NOT NULL,
    consumed_at       TIMESTAMPTZ
);

CREATE TABLE oauth_grants (
    id                BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    user_id           BIGINT      NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    client_id         TEXT        NOT NULL REFERENCES oauth_clients(client_id) ON DELETE CASCADE,
    instance_id       BIGINT      NOT NULL REFERENCES instances(id) ON DELETE CASCADE,
    resource          TEXT        NOT NULL,
    scope             TEXT        NOT NULL,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at        TIMESTAMPTZ
);
CREATE INDEX grants_user_idx     ON oauth_grants (user_id);
CREATE INDEX grants_instance_idx ON oauth_grants (instance_id);

CREATE TABLE oauth_codes (
    id                BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    code_hash         BYTEA       NOT NULL UNIQUE,
    grant_id          BIGINT      NOT NULL REFERENCES oauth_grants(id) ON DELETE CASCADE,
    redirect_uri      TEXT        NOT NULL,
    code_challenge    TEXT        NOT NULL,
    resource          TEXT        NOT NULL,
    wrapped_dek       BYTEA,                         -- cleared on use
    expires_at        TIMESTAMPTZ NOT NULL,
    used_at           TIMESTAMPTZ
);
