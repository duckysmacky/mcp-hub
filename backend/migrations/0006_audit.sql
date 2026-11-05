CREATE TABLE audit_events (
    id                BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    at                TIMESTAMPTZ NOT NULL DEFAULT now(),
    user_id           BIGINT      REFERENCES users(id) ON DELETE SET NULL,
    instance_id       BIGINT      REFERENCES instances(id) ON DELETE SET NULL,
    event             TEXT        NOT NULL,
    detail            JSONB                          -- NEVER values, arguments or tokens (a tool name is fine)
);
CREATE INDEX audit_user_at_idx ON audit_events (user_id, at);
