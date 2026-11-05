ALTER TABLE instances
    ADD COLUMN disabled_tools JSONB NOT NULL DEFAULT '[]'::jsonb,
    ADD COLUMN last_tools     JSONB NOT NULL DEFAULT '[]'::jsonb;  -- from the last tools/list, for the UI
