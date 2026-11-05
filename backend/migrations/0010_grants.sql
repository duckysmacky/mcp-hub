ALTER TABLE oauth_grants ADD COLUMN last_used_at TIMESTAMPTZ;
ALTER TABLE oauth_clients
    ADD COLUMN client_uri TEXT,                      -- self-reported
    ADD COLUMN logo_uri   TEXT;                      -- self-reported
