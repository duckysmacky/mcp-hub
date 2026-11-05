-- A user's own server definition (D37). NULL means the instance uses the catalog's `server_id`.
ALTER TABLE instances ADD COLUMN custom JSONB;
