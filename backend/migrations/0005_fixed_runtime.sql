-- The catalog fixes each server's runtime; instances no longer choose one (D28).
ALTER TABLE instances DROP COLUMN runtime;
