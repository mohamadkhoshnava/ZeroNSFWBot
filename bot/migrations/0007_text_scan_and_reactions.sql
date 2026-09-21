-- Judging what people write, and who reacts.
--
-- Three additions, none of which any existing group inherits.
--
-- `text_*` is the topic scan: a group names the subjects it does not want
-- discussed and how certain the bot must be before acting. It is answered by
-- TypeSafe's Jev, which returns a calibrated number per topic rather than
-- prose, so a percentage is a meaningful thing for an admin to set. Without a
-- JEV_API_KEY the column is inert and the scan never runs.
--
-- `ad_*` is the advertising guard, kept separate because it asks a different
-- question: a topic asks what a message is *about*, this asks what it is
-- *for*. A message selling nothing can still be about gambling, and an advert
-- can be about anything at all. Its default action is the mildest one the bot
-- has — most people who drop a referral link in a group are members rather
-- than spam accounts, and deleting their message without a word reads as a
-- malfunction.
--
-- `reaction_scan` needs no model at all. Reaction spam is the vector that
-- opened up once comment spam started being removed: the account reacts to
-- somebody else's message, which puts its avatar and name in front of the
-- group with nothing of its own to delete. The profile scan already answers
-- "is this a spam account"; this only points it at reactors too.
ALTER TABLE groups
    ADD COLUMN IF NOT EXISTS text_scan BOOLEAN NOT NULL DEFAULT FALSE,
    -- Only the subject this bot is about. Anything else is a moderation
    -- opinion a group has to state for itself — a debate group that inherited
    -- `politics` would have its own subject matter deleted on day one.
    ADD COLUMN IF NOT EXISTS text_topics JSONB NOT NULL DEFAULT '["sexual"]',
    -- Percent, like `threshold`. 70 because the rubric's midpoint is "touches
    -- on it in passing", and a group asking for sexual talk to be removed does
    -- not mean every message that mentions it.
    ADD COLUMN IF NOT EXISTS text_threshold SMALLINT NOT NULL DEFAULT 70,
    ADD COLUMN IF NOT EXISTS text_action TEXT NOT NULL DEFAULT 'delete',
    ADD COLUMN IF NOT EXISTS ad_scan BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN IF NOT EXISTS ad_threshold SMALLINT NOT NULL DEFAULT 75,
    ADD COLUMN IF NOT EXISTS ad_action TEXT NOT NULL DEFAULT 'warn',
    ADD COLUMN IF NOT EXISTS reaction_scan BOOLEAN NOT NULL DEFAULT FALSE;

-- Matching the constraints the other thresholds have carried since they were
-- added. Wrapped because Postgres has no ADD CONSTRAINT IF NOT EXISTS, so
-- re-running this file by hand stays harmless.
DO $$
BEGIN
    ALTER TABLE groups
        ADD CONSTRAINT groups_text_threshold_range
            CHECK (text_threshold BETWEEN 0 AND 100);
EXCEPTION
    WHEN duplicate_object THEN NULL;
END $$;

DO $$
BEGIN
    ALTER TABLE groups
        ADD CONSTRAINT groups_ad_threshold_range
            CHECK (ad_threshold BETWEEN 0 AND 100);
EXCEPTION
    WHEN duplicate_object THEN NULL;
END $$;
