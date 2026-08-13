-- Version the scoring pipeline that produced each cached score.
--
-- The cache key is the photo fingerprint, which only changes when the user
-- swaps their avatar. That is exactly right for detecting a changed photo and
-- exactly wrong for a changed *scorer*: after the two-stage cascade shipped,
-- every already-cached user would have kept their single-model score and never
-- been re-verified — so the anime false positives this release fixes would have
-- persisted for precisely the accounts already scanned.
--
-- Reads match on this column too, so a bump invalidates the cache without a
-- destructive TRUNCATE, and a rollback finds its own entries still valid.
ALTER TABLE scan_cache
    ADD COLUMN IF NOT EXISTS pipeline TEXT NOT NULL DEFAULT 'v1-fast-only';

-- Existing rows keep 'v1-fast-only' from the default, so they simply stop
-- matching once the code starts writing 'v2-verified'.
CREATE INDEX IF NOT EXISTS scan_cache_pipeline_idx ON scan_cache (pipeline);
