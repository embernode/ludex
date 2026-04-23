-- Schema version 2. Adds a key/value settings table for daemon-wide
-- configuration that the user can tweak from the GUI (gate
-- thresholds, idle handling, etc.).
--
-- Values are always stored as TEXT and parsed by the repository
-- layer. That keeps the on-disk representation opaque to schema
-- evolution — adding a new setting is a SettingsRepo method, never a
-- migration — and keeps the table shape trivially inspectable with a
-- sqlite3 client.

CREATE TABLE settings (
    key   TEXT NOT NULL PRIMARY KEY,
    value TEXT NOT NULL
) STRICT;

UPDATE schema_info SET value = '2' WHERE key = 'version';
