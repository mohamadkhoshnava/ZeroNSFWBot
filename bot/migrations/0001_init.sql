-- ZeroNSFWBot initial schema.
-- Telegram ids are 64-bit signed; chat ids for supergroups are negative.

-- ---------------------------------------------------------------- groups --
CREATE TABLE IF NOT EXISTS groups (
    chat_id                 BIGINT PRIMARY KEY,
    title                   TEXT,
    username                TEXT,

    lang                    TEXT        NOT NULL DEFAULT 'en',
    -- TRUE once an admin picks a language explicitly; auto-detection then
    -- stops overriding it on rename.
    lang_locked             BOOLEAN     NOT NULL DEFAULT FALSE,

    threshold               SMALLINT    NOT NULL DEFAULT 40,
    policy                  TEXT        NOT NULL DEFAULT 'nsfw_and_contact',
    -- Filter ids required by the `custom` policy.
    custom_filters          JSONB       NOT NULL DEFAULT '[]',
    action                  TEXT        NOT NULL DEFAULT 'ban',

    dry_run                 BOOLEAN     NOT NULL DEFAULT FALSE,
    -- Skip users who already have at least this many messages here. 0 = scan all.
    grace_messages          INTEGER     NOT NULL DEFAULT 5,
    delete_bot_messages     BOOLEAN     NOT NULL DEFAULT FALSE,
    bot_message_ttl_secs    INTEGER     NOT NULL DEFAULT 60,
    global_blocklist        BOOLEAN     NOT NULL DEFAULT TRUE,

    member_count            INTEGER     NOT NULL DEFAULT 0,
    is_active               BOOLEAN     NOT NULL DEFAULT TRUE,
    added_at                TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at              TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT groups_threshold_range CHECK (threshold BETWEEN 0 AND 100),
    CONSTRAINT groups_grace_nonneg    CHECK (grace_messages >= 0)
);

CREATE INDEX IF NOT EXISTS groups_active_idx ON groups (is_active) WHERE is_active;

-- ----------------------------------------------------------------- users --
CREATE TABLE IF NOT EXISTS users (
    user_id      BIGINT PRIMARY KEY,
    username     TEXT,
    first_name   TEXT,
    lang         TEXT        NOT NULL DEFAULT 'en',
    lang_locked  BOOLEAN     NOT NULL DEFAULT FALSE,
    -- TRUE once they /start the bot in private: the precondition for any DM.
    started_bot  BOOLEAN     NOT NULL DEFAULT FALSE,
    -- Set when Telegram reports the user blocked the bot, so broadcasts skip them.
    is_blocked   BOOLEAN     NOT NULL DEFAULT FALSE,
    first_seen   TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen    TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS users_reachable_idx
    ON users (user_id) WHERE started_bot AND NOT is_blocked;

-- --------------------------------------------------------- group members --
-- Only exists to power the grace window; deliberately not a full roster.
CREATE TABLE IF NOT EXISTS group_members (
    chat_id       BIGINT      NOT NULL,
    user_id       BIGINT      NOT NULL,
    message_count INTEGER     NOT NULL DEFAULT 0,
    first_seen    TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (chat_id, user_id)
);

-- ----------------------------------------------------- per-admin settings --
CREATE TABLE IF NOT EXISTS group_admin_prefs (
    chat_id   BIGINT  NOT NULL REFERENCES groups (chat_id) ON DELETE CASCADE,
    admin_id  BIGINT  NOT NULL,
    dm_notify BOOLEAN NOT NULL DEFAULT FALSE,
    PRIMARY KEY (chat_id, admin_id)
);

CREATE INDEX IF NOT EXISTS group_admin_prefs_notify_idx
    ON group_admin_prefs (chat_id) WHERE dm_notify;

-- ------------------------------------------------------------ detections --
CREATE TABLE IF NOT EXISTS detections (
    id           BIGSERIAL PRIMARY KEY,
    chat_id      BIGINT      NOT NULL,
    user_id      BIGINT      NOT NULL,
    message_id   INTEGER,
    -- Highest NSFW probability observed for this user, 0.0-1.0.
    score        REAL        NOT NULL,
    -- What the policy asked for: ban | delete | mute | report.
    verdict      TEXT        NOT NULL,
    banned       BOOLEAN     NOT NULL DEFAULT FALSE,
    deleted      BOOLEAN     NOT NULL DEFAULT FALSE,
    muted        BOOLEAN     NOT NULL DEFAULT FALSE,
    dry_run      BOOLEAN     NOT NULL DEFAULT FALSE,
    -- FALSE when an admin pressed "false positive".
    is_correct   BOOLEAN,
    -- [{"filter": "...", "score": 0.9, "detail": "..."}]
    reasons      JSONB       NOT NULL DEFAULT '[]',
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS detections_chat_time_idx ON detections (chat_id, created_at DESC);
CREATE INDEX IF NOT EXISTS detections_time_idx      ON detections (created_at DESC);
CREATE INDEX IF NOT EXISTS detections_user_idx      ON detections (user_id);

-- ------------------------------------------------------------------ bans --
CREATE TABLE IF NOT EXISTS bans (
    chat_id     BIGINT      NOT NULL,
    user_id     BIGINT      NOT NULL,
    score       REAL,
    reasons     JSONB       NOT NULL DEFAULT '[]',
    banned_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    unbanned_at TIMESTAMPTZ,
    unbanned_by BIGINT,
    PRIMARY KEY (chat_id, user_id)
);

-- Powers the shared-blocklist filter: count distinct groups where a ban stands.
CREATE INDEX IF NOT EXISTS bans_user_active_idx
    ON bans (user_id) WHERE unbanned_at IS NULL;

-- ------------------------------------------------------------ scan cache --
-- Keyed by user. `photo_fingerprint` is the concatenation of Telegram's
-- file_unique_ids, which are stable per photo, so a cache hit means the
-- profile has not changed and the stored score is still valid.
CREATE TABLE IF NOT EXISTS scan_cache (
    user_id           BIGINT PRIMARY KEY,
    photo_fingerprint TEXT        NOT NULL,
    nsfw_score        REAL        NOT NULL,
    has_photo         BOOLEAN     NOT NULL,
    bio               TEXT,
    ocr_text          TEXT,
    scanned_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS scan_cache_scanned_idx ON scan_cache (scanned_at);

-- --------------------------------------------------------------- appeals --
CREATE TABLE IF NOT EXISTS appeals (
    id          BIGSERIAL PRIMARY KEY,
    chat_id     BIGINT      NOT NULL,
    user_id     BIGINT      NOT NULL,
    status      TEXT        NOT NULL DEFAULT 'open',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    resolved_at TIMESTAMPTZ,
    resolved_by BIGINT
);

CREATE UNIQUE INDEX IF NOT EXISTS appeals_open_unique_idx
    ON appeals (chat_id, user_id) WHERE status = 'open';

-- ------------------------------------------------------------- audit log --
CREATE TABLE IF NOT EXISTS audit_log (
    id         BIGSERIAL PRIMARY KEY,
    chat_id    BIGINT,
    actor_id   BIGINT,
    action     TEXT        NOT NULL,
    payload    JSONB       NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS audit_log_chat_time_idx ON audit_log (chat_id, created_at DESC);

-- ------------------------------------------------------------ broadcasts --
CREATE TABLE IF NOT EXISTS broadcasts (
    id          BIGSERIAL PRIMARY KEY,
    author_id   BIGINT      NOT NULL,
    target      TEXT        NOT NULL,
    body        TEXT        NOT NULL,
    total       INTEGER     NOT NULL DEFAULT 0,
    sent        INTEGER     NOT NULL DEFAULT 0,
    failed      INTEGER     NOT NULL DEFAULT 0,
    started_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at TIMESTAMPTZ
);
