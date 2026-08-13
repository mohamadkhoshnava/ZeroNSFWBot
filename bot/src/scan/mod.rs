//! Gathers everything the filters need about one user, exactly once.
//!
//! This is the only place that talks to Telegram and the detector during a
//! scan. Filters read the resulting [`ScanContext`] synchronously, so adding a
//! filter never adds an API call — and a group whose policy ignores a signal
//! never pays to compute it.

use std::sync::Arc;

use teloxide::{
    net::Download,
    prelude::*,
    types::{ChatFullInfo, ChatFullInfoKind, ChatId, Message, PhotoSize, User, UserId},
};

use crate::{
    App, Tg,
    db::{self, models::GroupSettings},
    detector::{ImageItem, Verification},
    filters::Needs,
};

/// Everything known about the account behind one comment.
#[derive(Debug, Clone)]
pub struct ScanContext {
    pub user_id: i64,
    /// First and last name joined — what the group actually sees.
    pub display_name: String,
    pub username: Option<String>,

    /// `None` means the bio could not be read (privacy settings, or Telegram
    /// declined). Filters must treat that as unknown, never as empty.
    pub bio: Option<String>,
    /// The channel the account attached to its profile, if any.
    pub personal_channel: PersonalChannel,
    /// Whether the account has a profile photo the bot can see.
    pub photos: PhotoAccess,
    /// Highest NSFW probability across the scanned profile photos, with the
    /// working that produced it.
    pub profile_nsfw: Option<ImageScoring>,
    /// Text read off the avatar, when OCR is enabled and found something.
    pub avatar_text: Option<String>,
    /// NSFW probability of media attached to this specific message.
    pub message_nsfw: Option<ImageScoring>,
    /// Bans for this account in other groups. `None` when not looked up.
    pub other_group_bans: Option<i64>,

    pub settings: GroupSettings,
    pub reputation_min_bans: i64,
}

/// What the bot managed to learn about an account's profile photos.
///
/// This is deliberately three-valued rather than a `bool`. "I looked and there
/// is no photo" and "I did not look, or the lookup failed" are completely
/// different facts, and collapsing them means every transient
/// `getUserProfilePhotos` failure is indistinguishable from a genuinely
/// avatar-less account — which is exactly how an innocent user gets banned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhotoAccess {
    /// Telegram returned at least one photo.
    Visible,
    /// Telegram positively reported zero photos: no avatar, or one hidden by
    /// the user's privacy settings.
    Absent,
    /// Not looked up, or the lookup failed. Never a signal on its own.
    Unknown,
}

/// How an image's NSFW score was arrived at.
///
/// Both stages are kept, not just the final number, because "the fast model
/// said 88% and the verifier said 4%" and "both models said 88%" are very
/// different situations for an admin reviewing a ban — and with only the final
/// score they look identical.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageScoring {
    /// What the fast screening model said.
    pub fast: f32,
    /// The authoritative score: the verifier's when it ran, else the fast one.
    pub score: f32,
    /// Whether the second stage actually ran.
    pub verified: bool,
    /// The verifier's leading labels, strongest first. This is what shows an
    /// admin *why* — `drawings 76%` explains a cleared avatar at a glance.
    pub labels: Vec<(String, f32)>,
    /// Where in a clip this score came from, when the image was one frame of
    /// several. Absent for an ordinary still.
    pub sampled: Option<Sampled>,
}

/// Which frame of a sampled clip produced the score.
///
/// Worth reporting: "frame 4 of 5" tells an admin the GIF really was checked
/// through to the end, and that the part which triggered the ban is not the one
/// they see in the chat's preview.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sampled {
    /// 1-based position among the frames scored.
    pub frame: u32,
    /// How many were scored.
    pub of: u32,
}

impl ImageScoring {
    /// A score from the screening model alone, because it fell below the
    /// threshold and there was nothing to confirm.
    pub fn screened(score: f32) -> Self {
        Self {
            fast: score,
            score,
            verified: false,
            labels: Vec::new(),
            sampled: None,
        }
    }

    /// Note which frame of a clip this score came from.
    pub fn from_frame(mut self, frame: u32, of: u32) -> Self {
        self.sampled = Some(Sampled { frame, of });
        self
    }

    /// A verified score, derived from the classes *this group* counts.
    ///
    /// The same breakdown yields different numbers in different groups, which
    /// is the point: an art community counting only `porn` and a strict group
    /// also counting `sexy` are both right, and neither needs its own model.
    pub fn verified(fast: f32, labels: Vec<(String, f32)>, categories: &[String]) -> Self {
        let score = labels
            .iter()
            .filter(|(name, _)| categories.iter().any(|c| c.eq_ignore_ascii_case(name)))
            .map(|(_, value)| value)
            .sum();

        Self {
            fast,
            score,
            verified: true,
            labels,
            sampled: None,
        }
    }

    /// A compact, language-neutral summary for the detection report.
    ///
    /// Deliberately not translated: it is stored in the database with the
    /// detection and rendered long afterwards, possibly in a different language
    /// than the group had at the time. Percentages and an arrow read the same
    /// everywhere.
    pub fn detail(&self) -> String {
        let pct = |v: f32| crate::util::text::percent(v);

        let mut out = if self.verified {
            let mut out = format!("{}% → {}%", pct(self.fast), pct(self.score));
            if let Some((label, value)) = self.labels.first() {
                out.push_str(&format!(" · {label} {}%", pct(*value)));
            }
            out
        } else {
            format!("{}%", pct(self.score))
        };

        if let Some(sampled) = self.sampled {
            out.push_str(&format!(" · frame {}/{}", sampled.frame, sampled.of));
        }
        out
    }
}

/// The channel a user attached to their Telegram profile.
///
/// Telegram lets an account pin a channel to its profile, and it is displayed
/// there just like the bio. Spammers reach for it precisely because it is not
/// bio text: an account whose bio is empty or innocent can still advertise a
/// channel to everyone who opens the profile.
///
/// Three-valued for the same reason as [`PhotoAccess`]: "the profile has no
/// channel" and "the lookup failed" are different facts, and collapsing them
/// turns a transient `getChat` error into evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PersonalChannel {
    /// Telegram reported a channel on the profile.
    Linked(LinkedChannel),
    /// Telegram answered and there was no channel.
    Absent,
    /// Not looked up, or the lookup failed. Never a signal on its own.
    Unknown,
}

/// The parts of an attached channel worth showing in a detection report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedChannel {
    pub title: Option<String>,
    pub username: Option<String>,
}

impl LinkedChannel {
    /// How the channel is named in a report: its @handle when it is public,
    /// otherwise its title. Private channels have no handle, and an attached
    /// private channel is still advertising.
    pub fn label(&self) -> String {
        match (&self.username, &self.title) {
            (Some(username), _) => format!("@{username}"),
            (None, Some(title)) => title.clone(),
            (None, None) => "private channel".to_owned(),
        }
    }
}

impl ScanContext {
    /// A context for a detection that rests on the message's media alone.
    ///
    /// The group media scan never looks the account up — that is the point of
    /// it, and why it can run on everyone rather than just newcomers — so every
    /// profile signal here is `Unknown` rather than absent. Nothing may read
    /// this as "the bio was clean" or "there is no avatar", because nobody
    /// asked either question.
    pub fn media_only(
        user_id: i64,
        display_name: String,
        username: Option<String>,
        settings: GroupSettings,
        message_nsfw: ImageScoring,
        reputation_min_bans: i64,
    ) -> Self {
        Self {
            user_id,
            display_name,
            username,
            bio: None,
            personal_channel: PersonalChannel::Unknown,
            photos: PhotoAccess::Unknown,
            profile_nsfw: None,
            avatar_text: None,
            message_nsfw: Some(message_nsfw),
            other_group_bans: None,
            settings,
            reputation_min_bans,
        }
    }

    /// The group's threshold as a `0.0..=1.0` probability.
    pub fn threshold(&self) -> f32 {
        self.settings.threshold_ratio()
    }

    /// `true` only when the bot positively established there is no photo.
    pub fn photo_is_absent(&self) -> bool {
        self.photos == PhotoAccess::Absent
    }
}

/// Profile photos, reduced to what the scanner needs.
struct ProfilePhotos {
    /// Concatenated `file_unique_id`s — the cache key.
    fingerprint: String,
    /// One `PhotoSize` per photo: the largest variant available.
    largest: Vec<PhotoSize>,
}

/// Collect the data required by `needs` for `user_id`.
///
/// Telegram lookups run concurrently; a failure in any one of them degrades
/// that signal to "unavailable" rather than aborting the scan, because a
/// hidden bio must not become a free pass for an obviously NSFW avatar.
///
/// `already_scored` carries a score the group media scan has just computed for
/// this message's attachment, so the two passes never download it twice.
pub async fn collect(
    app: &Arc<App>,
    bot: &Tg,
    settings: GroupSettings,
    user: &User,
    message: &Message,
    needs: Needs,
    already_scored: Option<ImageScoring>,
) -> ScanContext {
    let chat_id = settings.chat_id;
    let user_id = user.id.0 as i64;
    // Both stages of the cascade compare against the same group threshold.
    let threshold = settings.threshold_ratio();

    let (profile, profile_chat, message_nsfw, other_group_bans) = tokio::join!(
        collect_profile(
            app,
            bot,
            user_id,
            needs,
            threshold,
            &settings.nsfw_categories
        ),
        collect_profile_chat(bot, user_id, needs),
        // The group media scan runs before this and looks at the very same
        // attachment, only more thoroughly — whole clips rather than one
        // thumbnail. Re-fetching it here would buy the same answer twice.
        async {
            match already_scored {
                Some(scoring) => Some(scoring),
                None => {
                    collect_message_media(
                        app,
                        bot,
                        message,
                        needs,
                        threshold,
                        &settings.nsfw_categories,
                        user_id,
                    )
                    .await
                }
            }
        },
        collect_reputation(app, user_id, chat_id, needs),
    );

    let (photos, profile_nsfw, avatar_text) = profile;
    let (bio, personal_channel) = profile_chat;

    ScanContext {
        user_id,
        display_name: display_name(user),
        username: user.username.clone(),
        bio,
        personal_channel,
        photos,
        profile_nsfw,
        avatar_text,
        message_nsfw,
        other_group_bans,
        settings,
        reputation_min_bans: app.cfg.global_reputation_min_bans,
    }
}

/// Returns `(photo access, scoring, avatar_text)`.
async fn collect_profile(
    app: &Arc<App>,
    bot: &Tg,
    user_id: i64,
    needs: Needs,
    threshold: f32,
    categories: &[String],
) -> (PhotoAccess, Option<ImageScoring>, Option<String>) {
    if !needs.profile_photos {
        // Nobody asked, so nothing is known — not "there is no photo".
        return (PhotoAccess::Unknown, None, None);
    }

    let photos = match fetch_profile_photos(app, bot, user_id).await {
        FetchedPhotos::Some(photos) => photos,
        // Telegram answered and the list was empty. A real, usable fact.
        FetchedPhotos::Empty => return (PhotoAccess::Absent, None, None),
        // The call failed. Reporting this as "no photo" is what turns a
        // network hiccup into a ban, so it stays Unknown.
        FetchedPhotos::Failed => return (PhotoAccess::Unknown, None, None),
    };

    // A cache hit means the fingerprint matched, so these exact photos have
    // already been scored and nothing needs downloading.
    if let Ok(Some(cached)) = db::scan_cache::get(&app.db, user_id, &photos.fingerprint).await {
        let ocr = cached.ocr_text.clone().filter(|t| !t.is_empty());

        // The cache holds evidence, not a verdict: the screening score plus the
        // verifier's breakdown. This group derives its own number from that,
        // so two groups with different categories reuse one scan and still get
        // different — correct — answers.
        let scoring = match cached.labels() {
            Some(labels) => Some(ImageScoring::verified(
                cached.nsfw_score,
                labels,
                categories,
            )),
            // Screened before but never escalated, because whichever group
            // scanned it had a higher threshold. If it is over *this* group's
            // threshold it still needs verifying, so fall through to a fresh
            // scan rather than acting on the screening score alone.
            None if cached.nsfw_score >= threshold => None,
            None => Some(ImageScoring::screened(cached.nsfw_score)),
        };

        if let Some(scoring) = scoring {
            return (PhotoAccess::Visible, Some(scoring), ocr);
        }
    }

    let mut images = Vec::with_capacity(photos.largest.len());
    for photo in &photos.largest {
        match download(bot, photo).await {
            Ok(bytes) => images.push(ImageItem::new(photo.file.unique_id.0.clone(), &bytes)),
            Err(err) => {
                tracing::debug!(user_id, %err, "could not download a profile photo");
            }
        }
    }

    if images.is_empty() {
        // The photos exist, we just could not download them.
        return (PhotoAccess::Visible, None, None);
    }

    let (scores, ocr) = tokio::join!(app.detector.classify(&images), async {
        if needs.avatar_text {
            app.detector.ocr(&images).await
        } else {
            Default::default()
        }
    });

    let fast = match scores {
        Ok(map) => map.values().copied().fold(f32::NEG_INFINITY, f32::max),
        Err(err) => {
            tracing::warn!(user_id, %err, "detector unavailable; profile score unknown");
            f32::NEG_INFINITY
        }
    };
    let fast = (fast > f32::NEG_INFINITY).then_some(fast);

    let nsfw = confirm(
        app, &images, fast, threshold, categories, "profile", user_id,
    )
    .await
    .map(|confirmed| confirmed.scoring);

    let avatar_text = {
        let joined = ocr.into_values().collect::<Vec<_>>().join(" ");
        (!joined.trim().is_empty()).then_some(joined)
    };

    if let Some(scoring) = &nsfw {
        let cache_write = db::scan_cache::put(
            &app.db,
            user_id,
            &photos.fingerprint,
            // Store the screening score, not the derived one: the derived score
            // belongs to this group's categories and the cache is shared.
            scoring.fast,
            true,
            None,
            avatar_text.as_deref(),
            scoring.verified.then_some(scoring.labels.as_slice()),
        )
        .await;
        if let Err(err) = cache_write {
            tracing::warn!(user_id, %err, "could not cache the profile scan");
        }
    }

    (PhotoAccess::Visible, nsfw, avatar_text)
}

/// Second stage of the cascade: re-score with the heavier model, but only for
/// images the fast one flagged.
///
/// The fast model has high recall and is cheap, which is exactly what you want
/// running on every comment — but it over-flags stylised art, and an anime
/// avatar is not pornography. The verifier is ~5x the parameters and, more
/// importantly, has separate `drawings` and `sexy` classes, so it can say "this
/// is a drawing" instead of "this is explicit".
///
/// Returns the score the rest of the pipeline should treat as authoritative:
///
/// * below the threshold → the fast score, unchanged (nothing to confirm)
/// * verified            → the verifier's score, which is what gets reported
/// * verifier missing    → `None`, i.e. *unknown*
///
/// That last case is deliberate. Falling back to the fast score would quietly
/// restore the false positives this exists to prevent, so a scan that cannot be
/// confirmed produces no image signal at all and no preset can act on it.
///
/// `images` may be several: the frames of one clip, or an account's avatars.
/// The worst of them wins, and [`Confirmed::image_id`] says which — that is how
/// a report can name the frame it acted on.
pub(crate) async fn confirm(
    app: &Arc<App>,
    images: &[ImageItem],
    fast: Option<f32>,
    threshold: f32,
    categories: &[String],
    what: &str,
    user_id: i64,
) -> Option<Confirmed> {
    let fast_score = fast?;

    // The overwhelming majority of scans stop here, which is what keeps the
    // heavy model affordable.
    if fast_score < threshold {
        return Some(Confirmed {
            scoring: ImageScoring::screened(fast_score),
            image_id: None,
        });
    }

    match app.detector.verify(images).await {
        Verification::Checked { scores, labels } => {
            // Score and labels must come from the *same* image, so pick the
            // worst-scoring one and read its breakdown, rather than taking a
            // max here and an arbitrary label set there.
            let worst = scores
                .iter()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .map(|(id, score)| (id.clone(), *score));

            let Some((id, verified)) = worst else {
                tracing::warn!(user_id, what, "verifier returned no usable score");
                return None;
            };

            let mut breakdown: Vec<(String, f32)> = labels
                .get(&id)
                .map(|m| m.iter().map(|(k, v)| (k.clone(), *v)).collect())
                .unwrap_or_default();
            breakdown.sort_by(|a, b| b.1.total_cmp(&a.1));

            let scoring = ImageScoring::verified(fast_score, breakdown, categories);

            // Logged at info because this is the decision an operator will want
            // to inspect when a ban looks wrong — or when one fails to happen.
            tracing::info!(
                user_id,
                what,
                fast = fast_score,
                detector_total = verified,
                group_score = scoring.score,
                categories = ?categories,
                labels = ?scoring.labels,
                "second-stage verification"
            );

            Some(Confirmed {
                scoring,
                image_id: Some(id),
            })
        }
        Verification::Unavailable => {
            tracing::warn!(
                user_id,
                what,
                fast = fast_score,
                "flagged but unverifiable; declining to treat it as a signal"
            );
            None
        }
    }
}

/// The outcome of [`confirm`]: a score, and which image produced it.
#[derive(Debug, Clone)]
pub(crate) struct Confirmed {
    pub scoring: ImageScoring,
    /// Id of the image the verifier scored worst. `None` when the batch never
    /// reached the second stage, where there is no single image to name.
    pub image_id: Option<String>,
}

/// The three outcomes of asking Telegram for a user's profile photos, kept
/// apart so a failure can never be mistaken for an empty result.
enum FetchedPhotos {
    Some(ProfilePhotos),
    Empty,
    Failed,
}

async fn fetch_profile_photos(app: &Arc<App>, bot: &Tg, user_id: i64) -> FetchedPhotos {
    let limit = app.cfg.defaults.profile_photos_to_scan;

    let photos = match bot
        .get_user_profile_photos(UserId(user_id as u64))
        .limit(limit as u8)
        .await
    {
        Ok(photos) => photos,
        Err(err) => {
            tracing::debug!(user_id, %err, "getUserProfilePhotos failed");
            return FetchedPhotos::Failed;
        }
    };

    // Telegram returns the currently displayed photo (the pinned one, if the
    // user pinned any) at index 0 and older ones after it, so taking the first
    // `limit` entries covers "current + previous" without extra requests.
    let largest: Vec<PhotoSize> = photos
        .photos
        .into_iter()
        .take(limit)
        .filter_map(|sizes| sizes.into_iter().max_by_key(|p| p.width * p.height))
        .collect();

    if largest.is_empty() {
        return FetchedPhotos::Empty;
    }

    FetchedPhotos::Some(ProfilePhotos {
        fingerprint: largest
            .iter()
            .map(|p| p.file.unique_id.0.as_str())
            .collect::<Vec<_>>()
            .join(":"),
        largest,
    })
}

/// Returns `(bio, personal channel)`.
///
/// Both come out of a single `getChat`, so a group that scans bios gets the
/// attached-channel signal for free — and vice versa.
async fn collect_profile_chat(
    bot: &Tg,
    user_id: i64,
    needs: Needs,
) -> (Option<String>, PersonalChannel) {
    if !needs.bio && !needs.personal_chat {
        return (None, PersonalChannel::Unknown);
    }

    // getChat against a user id returns their bio and their attached channel,
    // but only for users the bot can see and only when their privacy settings
    // allow it. Both failure modes land here as "unknown".
    let chat = match bot.get_chat(ChatId(user_id)).await {
        Ok(chat) => chat,
        Err(err) => {
            tracing::debug!(user_id, %err, "getChat for the profile failed");
            return (None, PersonalChannel::Unknown);
        }
    };

    let bio = needs.bio.then(|| chat.bio().map(str::to_owned)).flatten();

    let personal_channel = if needs.personal_chat {
        match personal_chat(&chat) {
            Some(channel) => PersonalChannel::Linked(channel),
            // getChat succeeded and carried no channel, so this is a real fact.
            None => PersonalChannel::Absent,
        }
    } else {
        PersonalChannel::Unknown
    };

    (bio, personal_channel)
}

/// Read the attached channel out of a `getChat` response.
///
/// `personal_chat` only exists on private chats; teloxide models that as a
/// variant rather than exposing an accessor, so the match is done here.
fn personal_chat(chat: &ChatFullInfo) -> Option<LinkedChannel> {
    let ChatFullInfoKind::Private(private) = &chat.kind else {
        return None;
    };

    let channel = private.personal_chat.as_deref()?;
    Some(LinkedChannel {
        title: channel.title().map(str::to_owned),
        username: channel.username().map(str::to_owned),
    })
}

async fn collect_message_media(
    app: &Arc<App>,
    bot: &Tg,
    message: &Message,
    needs: Needs,
    threshold: f32,
    categories: &[String],
    user_id: i64,
) -> Option<ImageScoring> {
    if !needs.message_media {
        return None;
    }

    let photo = message_image(message)?;
    let bytes = download(bot, &photo)
        .await
        .inspect_err(|err| tracing::debug!(%err, "could not download message media"))
        .ok()?;

    let images = [ImageItem::new(photo.file.unique_id.0.clone(), &bytes)];
    let fast = app
        .detector
        .classify(&images)
        .await
        .ok()?
        .into_values()
        .next();

    // Message media goes through the same two stages as an avatar. A shared
    // anime picture is the identical false positive, and it lands in a group
    // where everyone can see the bot got it wrong.
    confirm(
        app,
        &images,
        fast,
        threshold,
        categories,
        "message_media",
        user_id,
    )
    .await
    .map(|confirmed| confirmed.scoring)
}

/// The best still image in a message: a photo, or the thumbnail of a sticker,
/// animation or video. Thumbnails are enough — an explicit video has an
/// explicit first frame far more often than not, and scoring one small JPEG
/// beats downloading a 20 MB clip on every comment.
fn message_image(message: &Message) -> Option<PhotoSize> {
    if let Some(sizes) = message.photo() {
        return sizes.iter().max_by_key(|p| p.width * p.height).cloned();
    }
    if let Some(sticker) = message.sticker() {
        return sticker.thumbnail.clone();
    }
    if let Some(animation) = message.animation() {
        return animation.thumbnail.clone();
    }
    if let Some(video) = message.video() {
        return video.thumbnail.clone();
    }
    None
}

async fn collect_reputation(
    app: &Arc<App>,
    user_id: i64,
    chat_id: i64,
    needs: Needs,
) -> Option<i64> {
    if !needs.reputation {
        return None;
    }

    db::bans::other_group_bans(&app.db, user_id, chat_id)
        .await
        .inspect_err(|err| tracing::warn!(%err, "reputation lookup failed"))
        .ok()
}

/// First and last name joined — what the group actually sees next to a message.
pub fn display_name(user: &User) -> String {
    match &user.last_name {
        Some(last) => format!("{} {last}", user.first_name),
        None => user.first_name.clone(),
    }
}

async fn download(bot: &Tg, photo: &PhotoSize) -> anyhow::Result<Vec<u8>> {
    let file = bot.get_file(photo.file.id.clone()).await?;
    let mut buf = Vec::with_capacity(photo.file.size as usize);
    bot.download_file(&file.path, &mut buf).await?;
    Ok(buf)
}
