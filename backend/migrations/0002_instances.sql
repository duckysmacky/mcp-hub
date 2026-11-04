CREATE TABLE instances (
    id                BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    user_id           BIGINT      NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    slug              TEXT        NOT NULL CHECK (slug ~ '^[a-z0-9][a-z0-9-]{0,62}$'),
    server_id         TEXT        NOT NULL,          -- catalog [[server]].id
    runtime           TEXT        CHECK (runtime IN ('bun', 'node')),  -- NULL = catalog default
    enabled           BOOLEAN     NOT NULL DEFAULT false,
    needs_attention   TEXT,                          -- NULL | server_removed
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, slug)
);
CREATE INDEX instances_user_idx ON instances (user_id);

CREATE TABLE instance_env (
    instance_id       BIGINT      NOT NULL REFERENCES instances(id) ON DELETE CASCADE,
    env_var           TEXT        NOT NULL,
    ciphertext        BYTEA       NOT NULL,          -- nonce || ct || tag, AAD = env/v1 || instance_id || env_var
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (instance_id, env_var)
);
