# Changelog

Notable changes to ZeroNSFWBot. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added — banning the bots nobody promoted

Every filter in this project answers a question about a *person*: is this avatar
explicit, does this bio sell something, has this account been banned elsewhere.
A spam bot answers none of them. Its avatar is a logo, its bio is empty, its
name is unremarkable and its messages are plain text advertising, so there is no
image to score and no threshold that could ever fire. Groups were left removing
them by hand.

The signal that does hold is structural rather than content-based: **an
automation a group actually wanted is one an admin promoted.** So `/nsfw → 🤖
Ban unpromoted bots` bans any bot that arrives without administrator rights, and
a bot you want is one promotion away from being left alone permanently.

Off by default, and deliberately so — plenty of groups run an unpromoted helper
bot quite happily, and inheriting a rule that deletes it would be a nasty
surprise. `DEFAULT_BAN_FOREIGN_BOTS` sets what new groups start with.

Two things follow from how Telegram works, and both are limits worth stating
rather than bugs to fix later:

- **It acts on arrival, not on behaviour.** Telegram never delivers one bot's
  messages to another, so waiting for a spam bot to post is not an option — the
  update is not ours to receive. The switch watches the `new_chat_members`
  service message (sent by the human who did the adding) and `chat_member`
  updates (which cover an invite link). Registering the latter is also what
  makes teloxide ask Telegram for that update type at all.
- **It cannot sweep bots that were already there.** There is no "list members"
  call to enumerate them with, so turning the switch on affects arrivals from
  that moment onward.

`AdminSnapshot` now keeps promoted bots in their own `admin_bot_ids` list.
`is_admin` deliberately never counted them — it means "a human who can moderate
here" — and conflating the two to serve this feature would have quietly changed
who the settings panel answers to.

### Added — scanning the media posted in a group

Until now the bot only ever looked at a picture as evidence *about an account*.
`message_media` runs inside the profile scan, which means it inherits the grace
window, the clean-user cache and the profile threshold: a member past their
first few messages could post anything at all and never be looked at.

The group media scan is a separate section, off until an admin turns it on under
`/nsfw → 🖼 Media scan`, and it asks a different question — "is this picture
pornography?" rather than "is this account a spam profile?". Everything about it
follows from that:

- **It applies to everyone.** No grace window, no clean-user cache. An account's
  nature does not change between messages, so skipping a known-clean account is
  sound; a picture is not an account.
- **Its own threshold, defaulting to 90%** against the profile scan's 40%. That
  bar is right for one signal weighed against a bio, an avatar and a pinned
  channel, and reckless for deleting a long-standing member's photo on a model's
  opinion alone. Moving one threshold never moves the other.
- **Its own action, defaulting to delete-only** rather than the profile scan's
  ban. Removing an explicit picture is proportionate; banning the member who
  posted it is a separate decision, so it is a separate setting.
- **Per-kind switches** for photos, GIFs, stickers and videos, because their
  costs are nothing alike and a busy group may want stickers checked but not
  every forwarded clip.

**GIFs and videos are sampled, not thumbnailed.** Telegram re-encodes uploaded
GIFs to soundless MP4 and serves a poster thumbnail from an arbitrary frame, so
scoring that thumbnail scores a guess — and a clip that opens on a cat and ends
on pornography is a technique, not a hypothesis. The detector grew a `/frames`
endpoint that reduces a clip to stills spread across its whole length (Pillow
for GIF/WebP/APNG, ffmpeg for MP4/WebM); the bot scores every frame, keeps the
worst, and the detection detail names the frame it acted on:
`91% → 94% · porn 93% · frame 4/5`.

Three details that would otherwise bite:

- **One download, one score, two consumers.** When both passes want the same
  attachment, it is fetched once and verified at the *lower* of the two
  thresholds, so a score either pass would act on has always been through the
  second stage. Verifying at the media threshold alone would have handed the
  profile filter an unverified score to act on — reinstating the anime false
  positives the cascade exists to prevent.
- **A cached score is only reused when it can answer the question asked.** The
  cache is keyed by `file_unique_id` and shared across groups, so a *verified*
  entry serves anyone. An unverified one is only the screening model's opinion,
  kept because it fell below the bar in force when it was cached; a group with a
  lower bar treats it as a miss rather than acting on the fast score.
- **A clip that cannot be decoded is reported, not guessed at.** On a build
  without ffmpeg, `/frames` says `video_enabled: false` and the bot falls back to
  the thumbnail, rather than silently passing off one frame as a full check.

The ban notice sent to a removed user now says whether it was their profile or
their media that was flagged, and `report_score` no longer says "NSFW *profile*
probability" on a detection that had nothing to do with a profile.

Tuning: `DEFAULT_MEDIA_SCAN`, `DEFAULT_MEDIA_THRESHOLD`, `DEFAULT_MEDIA_ACTION`,
`DEFAULT_MEDIA_FRAMES` seed new groups; `MEDIA_MAX_DOWNLOAD_BYTES` caps what is
fetched whole; `ENABLE_VIDEO=false` builds a detector image without ffmpeg
(~150 MB smaller) and `DETECTOR_MAX_FRAMES` is the hard ceiling on frames per
clip whatever a group asks for.

### Added — two-stage image scoring

Reported as "the model is too sensitive to anime". It was not a tuning problem.
The screening model has two classes, `NSFW` and `SFW`; an anime portrait is not
`SFW`, so it has nowhere to go but `NSFW`. No threshold fixes a model that
cannot represent the distinction.

Image scoring is now a cascade. `Marqo/nsfw-image-detection-384` (ViT-tiny, 18M)
still screens every image, and anything it flags is re-scored by
`giacomoarienti/nsfw-classifier` (ViT-base, 86M) whose classes are `drawings`,
`hentai`, `neutral`, `porn` and `sexy`. Only `hentai` and `porn` count towards
the NSFW total, so explicit anime is still caught while ordinary anime is not.
Only the verified score is acted on.

- Both stages compare against the same per-group threshold, so the second stage
  can only clear an account the first flagged — never convict on its own.
- The heavy model runs on the flagged minority, which is what keeps a 5x larger
  model affordable at all.
- Message media goes through the same cascade; a shared anime picture is the
  same false positive, in front of the whole group.
- **A missing or unreachable verifier yields no image signal at all**, not a
  fallback to the fast score. Falling back would silently reinstate exactly what
  this prevents. CI asserts the verifier is loaded and that `drawings` is
  neither absent from its labels nor counted as explicit — a silently unexported
  model looks like "no more false positives" and means "no detection at all".
- Scores are cached with the pipeline version that produced them. The cache key
  is the photo fingerprint, which detects a changed *photo*, not a changed
  *scorer*; without the version, every already-scanned account would have kept
  its single-model verdict forever.
- The exporter now handles timm and transformers checkpoints, with a layered
  fallback for preprocessing: this model's `preprocessor_config.json` names
  `ViTFeatureExtractor`, a class transformers v5 removed, so `AutoImageProcessor`
  rejects a file whose values are perfectly readable.
- `HF_HUB_DISABLE_XET=1` — the hub's default transfer path aborts a 350 MB
  weight download mid-stream on an unreliable link instead of resuming.

Tuning: `VERIFIER_MODEL_ID` picks the model (empty builds without one),
`ENABLE_VERIFIER=false` turns the stage off, and `scripts/eval_images.sh` now
prints both columns side by side with the verifier's per-label breakdown.

### Added

- **`profile_channel` — the channel attached to a profile now counts as
  advertising.** Telegram lets an account pin a channel to its profile, shown
  there alongside the bio. Spam accounts had started leaving the bio empty or
  innocent and advertising through that instead, which every bio-based filter is
  blind to by construction — so the default `nsfw_and_contact` policy let an
  obviously NSFW profile through as long as it kept its bio clean.

  The new filter reads `personal_chat` off the `getChat` response the bio
  already comes from, so it costs no extra Telegram traffic. It joins `bio_link`
  and `profile_ocr` on the *contact* side of the decision: **NSFW + contact
  info** and **strict** now act on it, and it is selectable under **Custom**.

  It follows the same two rules as everything else. It never convicts alone — an
  ordinary person with their own channel is not a spammer, so an NSFW signal is
  still required. And it is three-valued: a failed `getChat` reports
  *unavailable* rather than "no channel", so an API error cannot satisfy the
  contact half of an AND. Under `strict` it shares the contact bucket with
  `bio_link` and `profile_ocr`, because someone who pins a channel usually links
  it too, and that is one fact rather than two.

### Fixed

- **The bot no longer claims to have removed a comment it did not remove.**
  A ban was issued with `revoke_messages`, and the code took that as proof the
  offending comment was gone — it never called `deleteMessage` in the ban path
  at all. When Telegram's sweep did not reach the comment, the group report, the
  admin DM and the `detections` row all recorded a deletion that never happened,
  while the comment sat in the group.

  The comment is now deleted first, by its own call, and every downstream claim
  comes from that call's result; `revoke_messages` stays on the ban to sweep the
  rest of the spam run. "Message to delete not found" counts as removed, since
  it means the comment is gone either way. New `report_banned_kept`,
  `report_muted_kept` and `dm_notify_kept` strings cover the case where the
  account was actioned but the comment survived.

### Fixed — moderation correctness

The entries below all came out of one incident: a real group member was banned
on a report that read **NSFW probability: 0%**. His bio contained his own
website and his avatar was not visible to the bot. Reviewing that one ban
surfaced the same underlying mistake — treating a weak or derived signal as
independent evidence — in three more places.

- **Never ban without NSFW evidence.** `no_photo_link` ("no visible avatar, but
  a link in the bio") could satisfy the `nsfw_and_contact` policy on its own. A
  policy named *NSFW + contact* was therefore firing with no NSFW signal at all,
  on a pattern that describes a great many ordinary, privacy-conscious users.
  It is removed from every preset and remains available only under **Custom**,
  where an admin combines it deliberately.

- **A failed lookup is not a signal.** Profile-photo availability was a `bool`,
  so "Telegram says this account has no photo" and "the lookup was skipped or
  failed" were the same value. Any transient `getUserProfilePhotos` error looked
  like positive evidence. It is now
  `PhotoAccess::{Visible, Absent, Unknown}`, and the filter fires only on
  `Absent` — the rule the bio filters already followed.

- **`strict` no longer counts one fact twice.** `bio_link` and `profile_ocr` are
  the same fact — the account publishes a way to reach it — written in two
  places. Counting them separately reached the two-signal bar with no NSFW
  evidence, so a business whose logo carries the website already in its bio
  would have been banned. They now share a bucket, alongside *image* and
  *vocabulary*.

- **Ban reports name only the signals the policy used.** Every filter runs, but
  the reasons list showed all of them. A `nsfw_and_contact` ban cited
  `no_photo_link`, which that policy ignores. Since the reasons list is what an
  admin uses to judge whether the bot was right, it now reflects only what the
  decision actually rested on.

### Fixed — everything else

- **Notifications arrived in English.** `lang_for` discarded a user's stored
  language unless they had pressed the language button, so the two call sites
  with no live `language_code` — both DM notifications — always fell through to
  English. A Persian admin's alert about a Persian group arrived in English.
  Adds `lang_for_group_notice`, which falls back to the *group's* language.

- **`/start` could panic.** `BOT_USERNAME` was parsed into a URL with
  `.expect()`, so a typo in the environment would crash the one screen every new
  user sees, on every press. Both configured URLs now degrade to a placeholder
  and a logged warning.

- **Appeals claimed a delivery that never happened.** An appeal reported "sent
  to the group admins" even when no admin had enabled DM alerts and nobody was
  notified. It now says so, and tells the user to contact an admin directly.

- **Docker builds no longer depend on a frontend fetch.** Dropped the
  `# syntax=docker/dockerfile:1` directive from both Dockerfiles. Neither uses a
  feature that needs it, and it forced a Docker Hub round trip on every build —
  a guaranteed failure on an unreliable link.

### Added

- **`/unban`** — lift a ban from the group, either as a reply to the user's
  message or with `/unban <user_id>` using the id from the report's 🔎 Details.
  Uses `only_if_banned`, so it cannot remove a current member by mistake.

- **Source link in the private chat.** `/start` and `/help` state that the bot
  is open source and carry a **Source code** button. Configurable via
  `PROJECT_URL` so a fork points at its own repository.

- **Maintainer credit.** A developer-channel line at the foot of `/start`,
  `/help` and the group welcome message — deliberately *not* on detection
  reports, where a credit under every ban would turn moderation into
  advertising. Configurable via `DEVELOPER_CHANNEL`; empty removes it.

- **Continuous integration.** `ci.yml` runs formatting, clippy, the Rust test
  suite, rustdoc, the detector's pytest suite, both Docker builds, and a
  compose-config check on every push and pull request. Two checks are specific
  to this project's failure modes: the built detector must score the committed
  SFW samples below 50% (a backwards ONNX label mapping would invert every
  verdict and nothing else would notice), and `.env.example` must match the
  variables `config.rs` actually reads in both directions.
- **`audit.yml`** — weekly `cargo audit` and a gitleaks secret scan, plus
  Dependabot for cargo, pip, Docker and Actions.

- **Dependencies brought current** — fastapi, uvicorn, pillow, numpy, pydantic,
  onnxruntime, timm, onnx and torch, plus four GitHub Actions.

  The torch 2.5 → 2.13 bump was not mechanical. torch ≥2.6 switched the
  `torch.onnx.export` default from the TorchScript tracer to dynamo, which
  imports `onnxscript`; the detector image stopped building on a bare
  `ModuleNotFoundError`. Adds `onnxscript` and states `dynamo=` explicitly, so a
  future torch release can only fail loudly rather than quietly export a
  different graph. `--no-dynamo` remains as an escape hatch.

  Verified equivalent rather than merely green: the dynamo-exported model scores
  the committed samples identically to the old one, to a tenth of a percent
  (33.4 / 16.8 / 16.2 / 8.3 / 6.1).

  Dependabot no longer proposes torch — it is build-stage only, its version is
  coupled to the export path, and it kept suggesting a `+cpu` local version that
  does not exist for every platform PyTorch publishes to, which breaks the arm64
  build.

### Changed

- `/info` counts private-chat users (`started_bot`) rather than every account
  ever seen in a group, which was dominated by passers-by and said nothing about
  the bot's reach.
- A settings change that affects detection now invalidates that group's
  clean-user cache, so a new threshold applies to the next message instead of up
  to 15 minutes later. Admins tune by watching what happens next; a lag reads as
  the setting not working.

### Known issues

- `groups::get_or_create` runs an upsert on **every** group message. It is
  correct but writes more than it needs to, and it rewrites `updated_at`
  constantly. A short-lived settings cache would fix both.
- Pending auto-deletions of the bot's own messages are lost on restart, so a
  report scheduled for deletion survives if the bot restarts within its TTL.

## [0.1.0] — 2026-08-07

First release.

- Async Telegram bot in Rust (teloxide) that scans the accounts behind comments
  and removes those advertising NSFW content.
- Eight filters over profile photos, avatar OCR, bio, display name, message
  media and cross-group reputation, behind an extensible `Filter` trait — adding
  one costs no extra API calls.
- Five policy presets plus a custom filter builder, per group.
- Separate CPU-only detector container: `Marqo/nsfw-image-detection-384`
  (ViT-tiny, ~18M parameters) exported to ONNX at image build time, with
  optional tesseract OCR.
- Four UI languages (English, فارسی, Русский, العربية), auto-detected from the
  group's title and description, with Persian as the tiebreaker for ambiguous
  Perso-Arabic text.
- `/nsfw` settings panel: threshold, filter mode, action, test mode, grace
  window, shared blocklist, per-admin DM alerts, statistics, language.
- Test mode, per-group grace windows, a false-positive button that unbans, and
  an appeal flow for removed users.
- Superadmin `/info` and a two-step, rate-limited `/broadcast`.
- PostgreSQL via sqlx with runtime-checked queries, so neither the build nor the
  test suite needs a live database.
