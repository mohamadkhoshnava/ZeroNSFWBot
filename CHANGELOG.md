# Changelog

Notable changes to ZeroNSFWBot. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
