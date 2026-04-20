-- Schema version 1. Subsequent migrations append to this sequence; existing
-- migrations are never edited once released.
--
-- All tables use STRICT type enforcement (SQLite 3.37+). Timestamps are
-- stored as TEXT in RFC 3339 (ISO 8601 UTC) form, which is lexicographically
-- orderable under ordinary B-tree indexes.

CREATE TABLE schema_info (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
) STRICT;

INSERT INTO schema_info (key, value) VALUES ('version', '1');

CREATE TABLE groups (
    id   INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT    NOT NULL UNIQUE
) STRICT;

INSERT INTO groups (name) VALUES
    ('Arcade'),
    ('Auto Simulator'),
    ('Fighting'),
    ('Flight Simulator'),
    ('First-Person Shooter'),
    ('Puzzle'),
    ('Quest'),
    ('Role-Playing Game'),
    ('Sport'),
    ('Standard'),
    ('Strategy'),
    ('Third-Person Shooter');

CREATE TABLE applications (
    id                     INTEGER PRIMARY KEY AUTOINCREMENT,

    -- Identity. (launcher_type, launcher_id) is the natural key; id is a
    -- stable surrogate used for foreign references.
    launcher_type          TEXT    NOT NULL
        CHECK (launcher_type IN ('steam','lutris','heroic','flatpak','native')),
    launcher_id            TEXT    NOT NULL,

    -- Display identity, resolved by the metadata enrichment cascade.
    product_name           TEXT    NOT NULL,
    publisher              TEXT,
    version                TEXT,

    -- Runtime identity — the paths ludex observes while the app is running.
    executable_path        TEXT,
    launcher_exe_path      TEXT,
    wineprefix_path        TEXT,
    installed_flatpak_ref  TEXT,

    -- Observed at runtime.
    graphics_platform      TEXT    NOT NULL DEFAULT 'unknown'
        CHECK (graphics_platform IN ('directx','opengl','vulkan','unknown')),
    process_architecture   TEXT    NOT NULL DEFAULT 'unknown'
        CHECK (process_architecture IN ('x86_64','i686','aarch64','unknown')),

    -- User-facing classification.
    group_id               INTEGER REFERENCES groups(id) ON DELETE SET NULL,

    -- Four standard icon sizes — populated during enrichment; may be NULL.
    icon_16                BLOB,
    icon_32                BLOB,
    icon_48                BLOB,
    icon_256               BLOB,

    -- Timestamps (RFC 3339 UTC).
    first_seen_at          TEXT    NOT NULL,
    last_played_at         TEXT,

    -- Aggregate statistics. Updated on session close.
    stat_run_count         INTEGER NOT NULL DEFAULT 0
        CHECK (stat_run_count >= 0),
    stat_total_full        INTEGER NOT NULL DEFAULT 0
        CHECK (stat_total_full >= 0),
    stat_total_interactive INTEGER NOT NULL DEFAULT 0
        CHECK (stat_total_interactive >= 0
               AND stat_total_interactive <= stat_total_full),
    stat_longest_full      INTEGER NOT NULL DEFAULT 0
        CHECK (stat_longest_full >= 0),

    UNIQUE (launcher_type, launcher_id)
) STRICT;

CREATE INDEX idx_applications_last_played ON applications(last_played_at DESC);

CREATE TABLE sessions (
    id                          INTEGER PRIMARY KEY AUTOINCREMENT,
    application_id              INTEGER NOT NULL
        REFERENCES applications(id) ON DELETE CASCADE,

    started_at                  TEXT    NOT NULL,
    ended_at                    TEXT,
    heartbeat_at                TEXT    NOT NULL,

    full_runtime_seconds        INTEGER NOT NULL DEFAULT 0
        CHECK (full_runtime_seconds >= 0),
    interactive_runtime_seconds INTEGER NOT NULL DEFAULT 0
        CHECK (interactive_runtime_seconds >= 0
               AND interactive_runtime_seconds <= full_runtime_seconds),

    exit_reason                 TEXT
        CHECK (exit_reason IS NULL OR exit_reason IN
              ('terminated','foreground_changed','recovered','sleep_split'))
) STRICT;

CREATE INDEX idx_sessions_app_started ON sessions(application_id, started_at DESC);
-- Partial index: open sessions only. Used by cold-start recovery.
CREATE INDEX idx_sessions_open ON sessions(application_id) WHERE ended_at IS NULL;

CREATE TABLE statistics_daily (
    date                        TEXT    PRIMARY KEY
        CHECK (length(date) = 10),  -- YYYY-MM-DD
    run_count                   INTEGER NOT NULL DEFAULT 0 CHECK (run_count >= 0),
    full_runtime_seconds        INTEGER NOT NULL DEFAULT 0 CHECK (full_runtime_seconds >= 0),
    interactive_runtime_seconds INTEGER NOT NULL DEFAULT 0
        CHECK (interactive_runtime_seconds >= 0
               AND interactive_runtime_seconds <= full_runtime_seconds)
) STRICT;

CREATE TABLE blocked_applications (
    launcher_type TEXT    NOT NULL
        CHECK (launcher_type IN ('steam','lutris','heroic','flatpak','native')),
    launcher_id   TEXT    NOT NULL,
    added_at      TEXT    NOT NULL,
    PRIMARY KEY (launcher_type, launcher_id)
) STRICT;

CREATE TABLE forced_applications (
    launcher_type TEXT    NOT NULL
        CHECK (launcher_type IN ('steam','lutris','heroic','flatpak','native')),
    launcher_id   TEXT    NOT NULL,
    added_at      TEXT    NOT NULL,
    PRIMARY KEY (launcher_type, launcher_id)
) STRICT;

CREATE TABLE emulators (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    product_name    TEXT    NOT NULL,
    executable_name TEXT    NOT NULL UNIQUE
) STRICT;

CREATE TABLE emulator_platforms (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    emulator_id   INTEGER NOT NULL REFERENCES emulators(id) ON DELETE CASCADE,
    platform_name TEXT    NOT NULL,
    UNIQUE (emulator_id, platform_name)
) STRICT;

CREATE TABLE emulator_platform_filename_patterns (
    id                   INTEGER PRIMARY KEY AUTOINCREMENT,
    emulator_platform_id INTEGER NOT NULL
        REFERENCES emulator_platforms(id) ON DELETE CASCADE,
    glob_pattern         TEXT    NOT NULL
) STRICT;
