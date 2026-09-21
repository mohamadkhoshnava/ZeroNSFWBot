# 🛡 ZeroNSFWBot

A Telegram moderation bot, written in async Rust, that detects and bans accounts
promoting NSFW content through comments, profiles, usernames, bios, avatars, or
hidden advertising.

**Features**

- Auto-detect NSFW advertisers from the profile photo, bio, name and attached channel
- Two-stage scoring: everything the fast model flags is re-checked by a stricter one
- Optional **group media scan** — the photos, GIFs, stickers and videos posted in
  the group, from every member, with its own threshold and its own action
- GIFs and videos sampled across the whole clip, not judged by their preview frame
- Optional **text scan** — judges how strongly a message is about subjects the
  group picks, with its own threshold and action
- Optional **ad guard** — catches messages whose purpose is to send people
  somewhere else, and warns rather than deletes by default
- Optional **reaction scan** — the accounts that react, not just the ones that write
- Instant auto-ban, or delete-only, or mute, or a public warning, or report — per group
- Reduce spam and keep communities clean

Four UI languages — English, فارسی, Русский, العربية — picked automatically from
the group's name and description, and changeable at any time.

---

## The problem it solves

In a channel's linked discussion group, spam accounts reply to posts with a
single emoji or a short greeting. The reply itself is harmless; the payload is
the *profile* next to it — an explicit avatar and a channel link in the bio.
Reading the message tells you nothing. You have to look at who sent it.

That is what this bot does, on every comment, automatically.

## How detection works

When someone comments, the bot gathers what Telegram already shows publicly and
runs it past a set of independent filters:

| Filter | Signal |
|---|---|
| `profile_nsfw` | NSFW score of the profile photos (current + previous), after two-stage verification |
| `message_media` | NSFW score of media inside the comment, after two-stage verification |
| `bio_link` | Link, `t.me/…` or `@username` in the bio |
| `profile_channel` | A channel attached to the profile — advertising the bio never mentions |
| `bio_keywords` | Adult advertising vocabulary in name, username or bio (4 languages) |
| `name_pattern` | Display name shaped like an ad — invite link, `18+`, `👇 click` |
| `profile_ocr` | Contact info written *onto* the avatar image |
| `no_photo_link` | No visible avatar, but a link in the bio — **not used by any preset**, see below |
| `bio_semantic` | The profile's text read as adult advertising by a language model — catches the obfuscated spellings and new slang a fixed word list cannot |
| `reputation` | Already banned for this in *N* other groups |

Each group's admins then choose which combination is enough to act on:

- **NSFW only** — the image alone.
- **NSFW + contact info** *(default)* — an explicit avatar **and** something
  being advertised, whether that is a link in the bio, contact details on the
  avatar, or a channel attached to the profile. The fewest false positives,
  because a person with a racy avatar and no channel to sell is not a spammer.
- **NSFW or keywords** — either the image or the ad copy.
- **Strict** — any two of three independent categories: an NSFW image, contact
  info (a bio link, the same link read off the avatar, or an attached channel —
  one fact, counted once), or advertising vocabulary.
- **Custom** — pick the exact filters yourself.

Two rules keep this honest, and both exist because breaking them produced real
false positives:

**Unavailable is never clean.** A filter that *cannot* run — a bio hidden by
privacy settings, a failed photo lookup — reports "unknown", not "no match". An
AND policy is never satisfied by a signal nobody could check.

**No preset convicts without NSFW evidence.** `no_photo_link` is deliberately in
no preset. "No avatar and a link in the bio" describes an enormous number of
ordinary, privacy-conscious users and contains no evidence of NSFW content at
all; it once banned a real group member on a report that read *NSFW probability:
0%*. It remains available under **Custom**, where an admin pairs it with
something else on purpose.

## The group media scan

Everything above answers *"is this account a spam profile?"*. The media scan
answers a different question — *"is this picture pornography?"* — and is a
separate switch, off until an admin turns it on under `/nsfw → 🖼 Media scan`.

The difference is not cosmetic. The profile scan runs inside the grace window
and skips anyone who recently scanned clean, because an account's nature does
not change between messages. A picture is not an account: a member of two years
can post one, so the media scan applies to everyone, every time, and never
consults the clean-user cache.

That independence is why it carries its own numbers:

| | Profile scan | Media scan |
|---|---|---|
| Threshold | 40% | **90%** |
| Action | Ban + delete | **Delete only** |
| Applies to | Members inside the grace window | **Everyone** |
| Enabled | Always | **Off by default** |

40% is a sensible bar for one signal weighed against a bio, an avatar and a
pinned channel. It is a reckless bar for deleting a long-standing member's
holiday photo on a model's opinion alone, which is why acting on an image by
itself demands near-certainty. Both numbers are editable, and moving one never
moves the other.

**Clips are sampled, not thumbnailed.** Telegram re-encodes every uploaded GIF
to a soundless MP4 and serves a poster thumbnail taken from an arbitrary frame.
Scoring that thumbnail is scoring a guess, and a clip that opens on something
innocuous and turns explicit later is a technique, not a hypothesis. The bot
downloads the clip and asks the detector's `/frames` endpoint for several stills
spread across its full length — Pillow for true GIF/WebP/APNG bytes, ffmpeg for
MP4 and WebM — then scores every frame and keeps the worst. The detection detail
names the frame it acted on (`91% → 94% · porn 93% · frame 4/5`), so an admin
looking at an innocent-looking preview can see why.

Cost is bounded at every step: a per-group frame count (default 5, hard ceiling
12), a file-size cap above which the thumbnail is used instead, per-kind
switches so a busy group can exclude videos, and a content-addressed cache keyed
by Telegram's `file_unique_id` — the same sticker posted a thousand times is
downloaded and scored once.

If the detector image was built without ffmpeg (`ENABLE_VIDEO=false`), `/frames`
says so rather than silently returning one frame, and the bot falls back to the
thumbnail.

## Architecture

```
┌──────────┐  long polling   ┌───────────────┐   HTTP    ┌──────────────────┐
│ Telegram │◄───────────────►│  bot (Rust)   │◄─────────►│ detector (Python)│
└──────────┘                 │   teloxide    │           │  ONNX + OCR      │
                             └───┬───────┬───┘           └──────────────────┘
                          sqlx   │       │  HTTPS, optional
                       ┌─────────▼──┐  ┌─▼──────────────────┐
                       │ PostgreSQL │  │ Jev (TypeSafe)     │
                       └────────────┘  │ typed text answers │
                                       └────────────────────┘
```

### Images stay local, text is optional

Two models, two boundaries. **Every image decision is made on your own
hardware** by the detector container — Jev takes no images at all, so nothing
about the NSFW pipeline changes whether it is configured or not.

Jev is a "System One" model: it returns a typed, calibrated number rather than
prose, evaluates every question in one parallel pass, and costs $0.042 per
million input tokens. That combination is what makes reading ordinary group
traffic affordable — a group watching six subjects pays for one call at roughly
the latency of asking one thing.

It powers four things, all of which degrade to "unavailable" with no
`JEV_API_KEY` set: the `bio_semantic` filter, the text scan, the ad guard, and
the group language guess. An outage is never read as "this text is clean", and
the regex word list stays in place underneath as a floor that needs no key.

The trade is privacy: with a key set, profile text — and, if you enable the text
scan, message text — is sent to a third-party API. Leave the key unset to keep
every scan on your own host.

### Two models, in a cascade

Image scoring runs in two stages, because one model cannot be both cheap enough
for every comment and precise enough to ban on.

| | Model | Runs on | Classes |
|---|---|---|---|
| **1. Screen** | [`Marqo/nsfw-image-detection-384`](https://huggingface.co/Marqo/nsfw-image-detection-384) — ViT-tiny, ~18M params | every scanned image | `NSFW` · `SFW` |
| **2. Verify** | [`giacomoarienti/nsfw-classifier`](https://huggingface.co/giacomoarienti/nsfw-classifier) — ViT-base, ~86M params | only what stage 1 flags | `drawings` · `hentai` · `neutral` · `porn` · `sexy` |

The fast model has high recall and costs a few tens of milliseconds, which is
what makes scanning every comment affordable. It also over-flags stylised art —
an anime avatar is not pornography, but a two-class model has nowhere to put it.

So anything it flags is re-scored by the verifier, and **only the verified score
is acted on**. The verifier is worth its cost less for the extra parameters than
for the taxonomy: `drawings` is a separate class from `hentai`, so it can say
*this is a drawing* instead of *this is explicit*. By default `porn`, `hentai`
and `sexy` count towards the NSFW total and `drawings` and `neutral` do not —
explicit anime is still caught, ordinary anime is not.

`sexy` is in the default set because that is the class the accounts this bot
exists to catch actually land in: an avatar explicit enough to score `porn` is
one Telegram removes on its own, so the advertising is done with lingerie, which
the verifier scores `sexy 99% / porn 1%`. Groups that want a racy avatar left
alone are already covered by the default preset, which needs advertising next to
the image before it acts. Each group can change the set from the panel.

Both stages compare against the same per-group threshold. The second stage can
only ever *clear* someone the first stage flagged, never convict on its own.

If the verifier is missing or unreachable, a flagged image produces **no image
signal at all** rather than falling back to the fast score. Falling back would
quietly reinstate exactly the false positives the cascade exists to prevent.

Both models are exported to ONNX at image build time and served by onnxruntime
on CPU. Torch, timm and transformers live only in a throwaway build stage, so
the runtime image ships none of them.

---

## Quick start

```bash
cp .env.example .env
```

Fill in at minimum:

- `TELEGRAM_BOT_TOKEN` — from [@BotFather](https://t.me/BotFather)
- `BOT_USERNAME` — your bot's username, without the `@`
- `SUPER_ADMINS` — your numeric user id (from [@userinfobot](https://t.me/userinfobot))
- `POSTGRES_PASSWORD` — anything, it stays inside the compose network

**Turn off Group Privacy in @BotFather** (`/mybots → Bot Settings → Group
Privacy → Turn off`). Without this the bot only receives commands, not the
comments it is supposed to scan.

```bash
docker compose up -d --build
docker compose logs -f bot
```

The first build downloads and exports the model, which takes a few minutes.
Afterwards it is cached.

### Setting up a group

1. Add the bot to the group.
2. Promote it with **Delete messages** and **Ban users**.
3. Send `/nsfw` and set the threshold and filter mode.
4. Leave **Test mode** on for a week, check `📊 Statistics`, then switch it off.

That last step matters. Test mode detects and reports without banning anyone, so
you can calibrate against your own group's members before the bot has the power
to remove them.

---

## Commands

| Command | Where | Who |
|---|---|---|
| `/start` | private | anyone |
| `/help` | private | anyone |
| `/nsfw` | group | admins — a non-admin's message is deleted, with no reply |
| `/unban <user_id>` | group | admins — or reply to the user's message with `/unban` |
| `/info` | private | super-admins |
| `/broadcast <users\|groups\|all> <text>` | private | super-admins |

`/broadcast` always shows a preview with an explicit confirm button, and paces
sending to stay inside Telegram's limits. It can also be used as a reply to the
message you want to send.

## The `/nsfw` panel

| | |
|---|---|
| 🎚 **Threshold** | Model confidence required, 0–100%. 30–50% suits most groups. |
| 🧩 **Filter mode** | Which combination of signals is actionable. |
| ⚡ **Action** | Ban + delete · Delete only · Mute + delete · Warn publicly · Report only. |
| 🕶 **Test mode** | Detect and report, change nothing. |
| 🛡 **Grace window** | Only scan members below N messages. Long-standing members are skipped, which cuts both false positives and CPU use. |
| 🖼 **Media scan** | Check the media posted in the group, from every member. Own sub-panel: on/off, its own threshold (90%), its own action (delete), which kinds to cover, and frames sampled per clip. **Off by default.** |
| 🗯 **Text scan** | Judge what people write. Pick the subjects — sexual content, violence, hate speech, insults, drugs, gambling, scams, politics, religion — then set how strongly a message must be *about* one (70%) and what to do. Needs a Jev key. **Off by default.** |
| 📣 **Ad guard** | Act on messages whose purpose is to send people elsewhere. Its own threshold (75%) and action (**warn publicly**, the mildest the bot has). A different question from the topics: a topic asks what a message is about, this asks what it is *for*. Needs a Jev key. **Off by default.** |
| 👍 **Reaction scan** | Run the profile checks on accounts that react to messages, not just those that write them. Nothing of anybody else's is touched — the message under the reaction is never deleted or replied to. Needs no key; needs the bot to be an admin. **Off by default.** |
| 🌍 **Shared blocklist** | Flag accounts banned for this in other groups, and contribute your own bans back. |
| 🔔 **My DM alerts** | Per-admin: get a private message on every removal. |
| 🧹 **Clean up my messages** | Auto-delete the bot's own reports after a TTL. **Off by default.** |
| 📊 **Statistics** | Detections, deletions and bans — daily, weekly, monthly, all time. |
| 🌐 **Language** | Overrides the automatic guess. |

Every detection report carries **❌ False positive**, which unbans the account
and records the mistake, and **🔎 Details**, which shows exactly which filters
fired and what they saw. Banned users get a private notice with an **Appeal**
button that routes to the admins.

---

## Adding a filter

The signal layer is designed to be extended. Adding one is four steps and costs
no extra API calls, because [`ScanContext`](bot/src/scan/mod.rs) gathers
everything once and filters read it synchronously.

1. Create `bot/src/filters/my_filter.rs`:

   ```rust
   use async_trait::async_trait;
   use super::{Filter, FilterOutcome, Needs};
   use crate::scan::ScanContext;

   pub struct MyFilter;

   #[async_trait]
   impl Filter for MyFilter {
       fn id(&self) -> &'static str { "my_filter" }

       // Only what you read. The scanner unions the needs of the filters the
       // active policy consults, so an unused signal costs nothing.
       fn needs(&self) -> Needs {
           Needs { bio: true, ..Needs::default() }
       }

       async fn evaluate(&self, ctx: &ScanContext) -> FilterOutcome {
           let Some(bio) = ctx.bio.as_deref() else {
               return FilterOutcome::unavailable();   // unknown, not clean
           };
           if bio.contains("something") {
               FilterOutcome::triggered(1.0, Some("evidence".into()))
           } else {
               FilterOutcome::not_triggered()
           }
       }
   }
   ```

2. Add `F_MY_FILTER => "my_filter"` to the `filter_ids!` list in
   [`bot/src/filters/mod.rs`](bot/src/filters/mod.rs).
3. Register it in `FilterRegistry::with_defaults`.
4. Add a `reason_my_filter` key to all four files in
   [`bot/locales/`](bot/locales/).

It then appears automatically under `/nsfw → Filter mode → Custom`. The test
suite enforces steps 2–4: `cargo test` fails if a declared filter is not
registered, or if any locale is missing its label.

---

## Development

```bash
make dev-infra                       # postgres + detector in Docker
cd bot && cargo run                  # the bot on the host, seconds per rebuild
```

Set `DATABASE_URL` to `…@localhost:5432/…` and `DETECTOR_URL` to
`http://localhost:8000` for the host-side run.

```bash
make lint          # cargo fmt --check + clippy -D warnings
make test          # Rust unit + integration tests
make detector-test # Python tests, in detector/.venv
```

### CI

[`.github/workflows/ci.yml`](.github/workflows/ci.yml) runs on every push and
pull request:

| Job | What it catches |
|---|---|
| **Rust** | `fmt --check`, `clippy -D warnings`, the full test suite, `cargo doc -D warnings` |
| **Detector** | `pytest` against a stubbed classifier — batching and per-image error isolation |
| **Images** | Both Dockerfiles build; the detector starts, loads the model, and scores the committed SFW samples |
| **Compose** | `docker compose config` resolves, and `.env.example` matches what `config.rs` actually reads |

Two of those are worth explaining. The image job asserts none of the synthetic
samples scores above 50% — if the ONNX export ever maps the class labels
backwards, every verdict the bot makes inverts, and nothing else would notice.
The compose job fails when a setting is added to `config.rs` without being
documented, or documented without being read, because a silently ignored
setting is worse than a missing one.

[`audit.yml`](.github/workflows/audit.yml) runs `cargo audit` and a gitleaks
scan weekly and whenever dependencies change. The secret scan is not decorative
here: the bot token and database password live in `.env`, and one careless
`git add -A` is all it takes.

Queries are runtime-checked rather than macro-checked, so neither the build nor
the test suite needs a live database.

### Evaluating the model

```bash
docker compose up -d detector
make gen-images                      # synthetic SFW samples
./scripts/eval_images.sh
```

The script scores every image under `assets/test_images/` and, if you supply
labelled positives in `assets/test_images/nsfw/`, prints accuracy and the
false-positive/false-negative counts at each candidate threshold. That folder is
empty and gitignored by design — this repository ships no adult material. See
[`assets/test_images/README.md`](assets/test_images/README.md).

---

## Operational notes

**Bios are not always readable.** `getChat` returns a user's bio only when their
privacy settings allow it. Filters that depend on it report *unavailable*, which
never satisfies an AND policy.

**The bio and the attached channel come from one call.** `getChat` carries both
`bio` and `personal_chat`, so `profile_channel` adds no Telegram traffic on top
of `bio_link`. Like the photo lookup it is three-valued — a channel, positively
no channel, or a failed call — and only the first two are facts.

**Photo lookups are three-valued.** `PhotoAccess` distinguishes *Visible*,
*Absent* (Telegram answered, there are none) and *Unknown* (not looked up, or
the call failed). Collapsing the last two into a boolean is what let a transient
`getUserProfilePhotos` error look like the positive signal "this account has no
avatar" — and ban someone for it.

**Pinned vs. current avatar.** The Bot API does not mark which profile photo is
pinned; it returns the displayed one first. The bot scans the newest
`PROFILE_PHOTOS_TO_SCAN` (default 2) and takes the highest score, which covers
both without extra requests.

**False positives happen.** Artistic and anime avatars were the usual cause,
which is what the verifier stage addresses — but it reduces them, it does not
eliminate them.
Test mode, the grace window, `/unban`, the false-positive button and appeals are
all defaults for that reason. Do not skip the calibration week.

**One bot, one token.** Two instances long-polling the same token fight over
every update: each keeps terminating the other's `getUpdates`, and settings and
statistics split across two databases. If the logs show
`TerminatedByOtherGetUpdates`, another copy is running somewhere — find it
before debugging anything else.

**The verifier is not free.** It is ~5x the parameters of the screening model
and runs on the small fraction of images that get flagged. On a group where most
comments come from clean accounts that is a rounding error; on a group under an
active spam run it is the dominant cost. `ENABLE_VERIFIER=false` disables it,
but then flagged images produce no signal at all — the bot declines to act
rather than trusting the model that over-flags.

**Caching.** Profile scans are keyed by Telegram's `file_unique_id`, which
changes the moment a user swaps their avatar — so a cache hit provably refers to
the same image, and a spammer cannot hide behind a stale score. Cached rows also
record which scoring pipeline produced them, so changing the models invalidates
them instead of serving verdicts from a scorer that no longer exists. Users who
scan clean are skipped for 15 minutes per group.

**Permissions.** Ban and delete need `can_restrict_members` and
`can_delete_messages`. When either is missing the bot says so in the report and
replies to the offending message, since that message is still there.

## Privacy

The bot reads only what Telegram already exposes publicly. It stores scores,
counts and the evidence strings shown in the Details view — not photos. The
private-chat detector test stores nothing at all. Images are sent to the local
detector container over the compose network and never leave your host.

**Text is different, and only if you opt in.** Setting `JEV_API_KEY` sends
profile text to TypeSafe's API, and enabling the text scan or ad guard sends
message text as well. Both are clipped to 600 characters and sent as structured
fields rather than concatenated prose. With no key set, nothing leaves your host
at all and every text feature reports itself unavailable.

Changes are recorded in [CHANGELOG.md](CHANGELOG.md).

## License

MIT
