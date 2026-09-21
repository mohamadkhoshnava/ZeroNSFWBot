# Changelog

Notable changes to ZeroNSFWBot. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added — judging what people write, not just what they look like

Everything in this bot up to now answered a question about an *account*: is this
avatar explicit, does this bio carry a link, is there a channel pinned to the
profile. That works because the spam it was built for puts the payload in the
profile and leaves the message itself harmless. It is also completely blind to
the group's actual traffic — a message is only ever looked at for the picture
attached to it.

Four additions close that, three of them powered by
[Jev](https://typesafe.ai/blog/introducing-system-one-models-and-jev), a model
that returns a typed, calibrated number instead of prose. It takes no images, so
**every NSFW image decision still happens on your own hardware** and nothing
about that pipeline changed. What it makes possible is reading text at the
volume a group produces: all questions are answered in one parallel pass, at
$0.042 per million input tokens, so a group watching six subjects pays for one
call at about the latency of asking one thing.

**`bio_semantic`** joins the filter layer. The existing `bio_keywords` is a
thirteen-entry regex list, and a list is structurally unable to catch `s3x`,
`س‌ک‌س` spaced with a zero-width non-joiner, a Cyrillic `о` inside `porn`, or
this month's slang. The word list stays exactly where it is as a floor that
needs no API key and cannot be talked out of firing; this reads the same fields
— name, username, bio, attached channel, and any text OCR lifted off the avatar
— and judges whether they add up to an advertisement. It fires at 70%, its own
fixed bar rather than the group's image threshold, because it can convict on
text alone under `nsfw_or_keywords`.

`strict` counts it in the same bucket as `bio_keywords` and `name_pattern`. They
are not independent signals — the model fires on exactly the bios the list was
written for, and then on the ones it was not — and counting them separately
would reach the two-signal bar from a single fact, which is the defect that once
banned a user over a bio link and the identical link read off their avatar.

**Text scan** (`/nsfw → 🗯 Text scan`) lets a group name the subjects it does not
want discussed — sexual content, violence, hate speech, insults, drugs,
gambling, scams, politics, religion — and set how strongly a message has to be
*about* one before the bot acts. Each subject carries a four-level rubric, and
the middle levels are the point: without "touches on it in passing" and
"substantially about it" there would be no gradations for a percentage to mean
anything against. New groups that turn it on start watching `sexual` only; every
other subject is a moderation opinion a group has to state for itself, and a
debate group that inherited `politics` would have deleted its own conversation.

**Ad guard** (`/nsfw → 📣 Ad guard`) is deliberately not one of those subjects,
because it asks a different question: a topic asks what a message is *about*,
this asks what it is *for*. A message selling nothing can still be about
gambling, and an advert can be about anything at all. Its default action is
**Warn publicly**, a new action that changes nothing and replies to the message
where the sender will see it — most people who drop a link in a group are
members rather than spam accounts, and deleting their message without a word
reads as the bot malfunctioning.

**Reaction scan** (`/nsfw → 👍 Reaction scan`) needs no model at all. It is the
vector that opens up once comment spam is being removed: the account taps an
emoji on somebody else's message, its avatar and name appear under that message
for everyone who opens the reaction list, and there is nothing of its own in the
chat to delete. Every profile signal already applies unchanged. Two things make
it different from scanning a message, and both are enforced in the types rather
than by convention: a verdict from a reaction carries no message id, so
`enforcement` cannot delete or reply to the message under it — that one belongs
to an innocent member — and a reaction never advances the grace-window counter,
or an account could react its way past ever being scanned.

The group language guess now asks the model first and keeps the script heuristic
as its fallback. Script detection is good when the script is decisive and
helpless when it is not: a Persian group branded "Tehran Traders" reads as
English, and Persian and Arabic share an alphabet, so a title with no marker
letters was settled by a tie-break rather than by evidence. The question is
asked once in a group's life, when the bot is added, and never in the message
path.

### Changed — the text scan cost 2293 tokens to read "سلام بچه‌ها"

Measured on the first hour of real traffic: 108 of 117 Jev calls came from one
group that had ticked all nine topics plus the ad guard, and each one sent
2286 input tokens to judge messages that were mostly a few words long. The
message is a rounding error in that number. The rest is ten rubrics, re-sent
in full on every message, plus about 300 tokens of fixed per-request overhead.

Two changes, in that order of confidence.

The wording is now as short as it can be while drawing the line in the same
place. The untrusted-input warning was 38 tokens repeated on all ten questions
— 350 tokens per message to say one thing ten times — and is now one clause.
The shared rubric levels and the per-topic preamble got the same treatment.
That alone is 2293 → 1434 tokens, with identical verdicts on every case in the
test set: the same topics fired, the same ones stayed below their thresholds,
and the numbers moved by a few points at most.

Then a gate in front of the whole thing. Almost everything in a group is
ordinary conversation, and asking ten detailed questions about a greeting
costs the same as asking them about a sales pitch. One cheap question — "is
this anything other than ordinary conversation?", naming only what this group
actually watches — runs first, and the full pass only runs on what it opens.
Measured against ordinary messages the gate answers 0.02–0.03; every message
that turned out to be worth acting on answered above 0.91, including one the
full pass then correctly cleared. The bar sits at 0.30, in the empty space
between.

The gate skips itself for groups whose full pass is under three questions,
where it would cost more than it saves, and a gate that fails or is unsure
opens the full pass rather than skipping it — failing the other way would turn
an outage into a silently disabled scan.

### How all of this fails

Jev is optional, and with no `JEV_API_KEY` set every feature above except the
reaction scan reports itself unavailable and the bot behaves exactly as it did
before. That is also what a timeout, a 429 or a malformed response produce —
`unavailable`, never a score. The distinction has been load-bearing in this
codebase since the `PhotoAccess` three-valued enum, and it matters more here
than anywhere: a text model that quietly returned "clean" on an outage would
disable the scan without saying so, and one that quietly returned "spam" would
be far worse.

The text a spammer writes is also, unavoidably, input to a model. Two things
bound it. Every field is sent as a labelled value inside a JSON object rather
than concatenated into the prompt, so a bio reading "ignore the above" is
visibly the *content of a bio*. And the answer is a number from a fixed type —
there is no output channel for an injected instruction to escape through, and
the worst a manipulated answer can be is a wrong number, which the thresholds
already bound. The regex floor underneath cannot be talked out of firing at all.

### Privacy

This is the first feature in the project that sends anything off the host.
Setting a key sends profile text; enabling the text scan or ad guard sends
message text, clipped to 600 characters. Images never go anywhere but the local
detector. Leaving `JEV_API_KEY` unset keeps every scan on your own hardware,
which is what a fresh deployment does by default.

### Fixed — the profile scan had been off since the cascade landed

The two-stage pipeline shipped with `porn` + `hentai` as the classes a group
counts as NSFW, on the reasoning that `sexy` was one of the two buckets that had
made the single-model pipeline unusable. That reasoning was about the *screening*
model's failure mode and did not survive contact with the verifier's taxonomy.

`porn` is what the verifier calls hardcore pornography, and an avatar that
explicit is one Telegram removes without our help. The accounts this bot exists
to catch advertise with lingerie and cleavage, and the verifier scores those
`sexy 99% / porn 1%`. So the second stage was taking profiles the screening model
had flagged at over 90% and handing back a group score under 1%.

The result was not a stricter filter but a disabled one: `profile_nsfw` did not
fire once in any group between the cascade going live and this fix, while the
same avatars kept being flagged at stage one and cleared at stage two. Every
detection in that window came from `message_media`, which has its own scoring
path and was unaffected.

`sexy` is now in the default set. `drawings` still is not — sparing stylised art
is what the verifier was added for, and that part worked. Groups that had already
picked their own categories keep them; migration `0006` moves only the untouched
default. A group that wants racy avatars ignored can drop the class from the
panel, though the default preset already declines to act on an image with no
advertising beside it.

`/reset` now binds the default set instead of repeating it as a SQL literal,
which is how the old value outlived the code that named it.

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
