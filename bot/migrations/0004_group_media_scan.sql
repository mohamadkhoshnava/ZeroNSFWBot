-- Scanning the media posted in a group, as its own section.
--
-- The existing `message_media` filter only ever looked at newcomers: it runs
-- inside the profile scan, so the grace window and the clean-user cache both
-- apply to it, and it shares the profile threshold. That is right for "is this
-- account a spam profile" and wrong for "is this picture pornography" — a
-- member of two years can post one, and 40% confidence is nowhere near enough
-- to act on a single image in isolation.
--
-- So this is deliberately a separate switch with its own numbers: off by
-- default, applied to everyone, and defaulting to a much higher bar.
ALTER TABLE groups
    ADD COLUMN IF NOT EXISTS media_scan BOOLEAN NOT NULL DEFAULT FALSE,
    -- Percent, like `threshold`. 90 rather than 40 because acting on a single
    -- posted image needs near-certainty; the profile threshold is one signal
    -- among several, this one stands alone.
    ADD COLUMN IF NOT EXISTS media_threshold SMALLINT NOT NULL DEFAULT 90,
    -- Separate from `action` on purpose. Deleting an explicit picture from a
    -- long-standing member is proportionate; banning them for it is a decision
    -- an admin should have to make deliberately.
    ADD COLUMN IF NOT EXISTS media_action TEXT NOT NULL DEFAULT 'delete',
    -- Which kinds are worth the download in this group. Videos are the
    -- expensive ones, so a group can drop them and keep the rest.
    ADD COLUMN IF NOT EXISTS media_kinds JSONB NOT NULL
        DEFAULT '["photo", "animation", "sticker", "video"]',
    -- Stills sampled across an animation or clip. One frame is a guess: the
    -- thumbnail Telegram serves is an arbitrary frame and spam GIFs routinely
    -- open on something innocuous.
    ADD COLUMN IF NOT EXISTS media_frames SMALLINT NOT NULL DEFAULT 5;

-- Matching the constraint `threshold` has carried since the initial schema.
-- Wrapped so re-running this file by hand is harmless: Postgres has no
-- ADD CONSTRAINT IF NOT EXISTS.
DO $$
BEGIN
    ALTER TABLE groups
        ADD CONSTRAINT groups_media_threshold_range
            CHECK (media_threshold BETWEEN 0 AND 100);
EXCEPTION
    WHEN duplicate_object THEN NULL;
END $$;
