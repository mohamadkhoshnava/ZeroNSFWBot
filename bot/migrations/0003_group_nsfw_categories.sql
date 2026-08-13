-- Let each group choose which of the verifier's classes count as NSFW.
--
-- Previously this was baked into the detector image at build time, so every
-- group shared one answer. It is not one answer: an art community wants
-- `drawings` ignored forever, a family group may want `sexy` acted on, and
-- neither is wrong. It belongs next to the threshold and the filter mode.
ALTER TABLE groups
    ADD COLUMN IF NOT EXISTS nsfw_categories JSONB NOT NULL DEFAULT '["porn", "hentai"]';

-- The scan cache is keyed by user and shared across every group, so it can no
-- longer store a *score* — that score is now group-dependent. It stores the
-- verifier's per-class breakdown instead, and each group derives its own number
-- from it. One scan, many verdicts.
--
-- NULL means the account was screened but never escalated: some group's
-- threshold was high enough that the second stage did not run. The next group
-- with a lower threshold verifies it and fills this in.
ALTER TABLE scan_cache
    ADD COLUMN IF NOT EXISTS verifier_labels JSONB;
