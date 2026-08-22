-- Count `sexy` as NSFW by default.
--
-- The two-stage pipeline shipped with a default of `porn` + `hentai`, on the
-- theory that those are the classes nobody argues about. In practice they are
-- the classes that almost never fire: an avatar explicit enough to score `porn`
-- is one Telegram removes on its own, so the accounts this bot exists to catch
-- advertise with lingerie instead — which the verifier calls `sexy` at 99% and
-- `porn` at well under 1%.
--
-- The effect was not a stricter filter but a disabled one. Between the second
-- stage going live and this migration, `profile_nsfw` did not trigger once in
-- any group, while the screening model went on flagging the same avatars at
-- over 90%.
--
-- Groups that had already opened the categories screen and picked something
-- else keep their choice; only the untouched default moves.
ALTER TABLE groups
    ALTER COLUMN nsfw_categories SET DEFAULT '["porn", "hentai", "sexy"]';

UPDATE groups
SET nsfw_categories = '["porn", "hentai", "sexy"]',
    updated_at = now()
WHERE nsfw_categories = '["porn", "hentai"]'::jsonb;
