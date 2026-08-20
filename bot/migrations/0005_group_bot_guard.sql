-- Banning the other bots a group did not ask for.
--
-- Spam rings add their own bot to a group and let it advertise on a schedule,
-- which no NSFW signal can catch: the avatar is a logo, the bio is empty, and
-- the messages are text. The one thing that is always true is that nobody made
-- it an admin — a bot an admin actually wanted is a bot an admin promoted.
--
-- Off by default. Plenty of groups run a legitimate helper bot without giving
-- it admin rights, and inheriting a rule that bans it would be a nasty
-- surprise; turning this on is a deliberate statement about the group.
ALTER TABLE groups
    ADD COLUMN IF NOT EXISTS ban_foreign_bots BOOLEAN NOT NULL DEFAULT FALSE;
