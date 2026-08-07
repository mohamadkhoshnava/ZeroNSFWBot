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
    types::{ChatId, Message, PhotoSize, User, UserId},
};

use crate::{
    App, Tg,
    db::{self, models::GroupSettings},
    detector::ImageItem,
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
    /// Whether the account has a profile photo the bot can see.
    pub photos: PhotoAccess,
    /// Highest NSFW probability across the scanned profile photos.
    pub profile_nsfw: Option<f32>,
    /// Text read off the avatar, when OCR is enabled and found something.
    pub avatar_text: Option<String>,
    /// NSFW probability of media attached to this specific message.
    pub message_nsfw: Option<f32>,
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

impl ScanContext {
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
pub async fn collect(
    app: &Arc<App>,
    bot: &Tg,
    settings: GroupSettings,
    user: &User,
    message: &Message,
    needs: Needs,
) -> ScanContext {
    let chat_id = settings.chat_id;
    let user_id = user.id.0 as i64;

    let (profile, bio, message_nsfw, other_group_bans) = tokio::join!(
        collect_profile(app, bot, user_id, needs),
        collect_bio(bot, user_id, needs),
        collect_message_media(app, bot, message, needs),
        collect_reputation(app, user_id, chat_id, needs),
    );

    let (photos, profile_nsfw, avatar_text) = profile;

    ScanContext {
        user_id,
        display_name: display_name(user),
        username: user.username.clone(),
        bio,
        photos,
        profile_nsfw,
        avatar_text,
        message_nsfw,
        other_group_bans,
        settings,
        reputation_min_bans: app.cfg.global_reputation_min_bans,
    }
}

/// Returns `(photo access, nsfw_score, avatar_text)`.
async fn collect_profile(
    app: &Arc<App>,
    bot: &Tg,
    user_id: i64,
    needs: Needs,
) -> (PhotoAccess, Option<f32>, Option<String>) {
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
        return (
            PhotoAccess::Visible,
            Some(cached.nsfw_score),
            cached.ocr_text.filter(|t| !t.is_empty()),
        );
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

    let nsfw = match scores {
        Ok(map) => map.values().copied().fold(f32::NEG_INFINITY, f32::max),
        Err(err) => {
            tracing::warn!(user_id, %err, "detector unavailable; profile score unknown");
            f32::NEG_INFINITY
        }
    };
    let nsfw = (nsfw > f32::NEG_INFINITY).then_some(nsfw);

    let avatar_text = {
        let joined = ocr.into_values().collect::<Vec<_>>().join(" ");
        (!joined.trim().is_empty()).then_some(joined)
    };

    if let Some(score) = nsfw {
        let cache_write = db::scan_cache::put(
            &app.db,
            user_id,
            &photos.fingerprint,
            score,
            true,
            None,
            avatar_text.as_deref(),
        )
        .await;
        if let Err(err) = cache_write {
            tracing::warn!(user_id, %err, "could not cache the profile scan");
        }
    }

    (PhotoAccess::Visible, nsfw, avatar_text)
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

async fn collect_bio(bot: &Tg, user_id: i64, needs: Needs) -> Option<String> {
    if !needs.bio {
        return None;
    }

    // getChat against a user id returns their bio, but only for users the bot
    // can see and only when their privacy settings allow it. Both failure modes
    // land here as `None`, i.e. "unknown".
    let chat = bot
        .get_chat(ChatId(user_id))
        .await
        .inspect_err(|err| tracing::debug!(user_id, %err, "getChat for bio failed"))
        .ok()?;

    chat.bio().map(str::to_owned)
}

async fn collect_message_media(
    app: &Arc<App>,
    bot: &Tg,
    message: &Message,
    needs: Needs,
) -> Option<f32> {
    if !needs.message_media {
        return None;
    }

    let photo = message_image(message)?;
    let bytes = download(bot, &photo)
        .await
        .inspect_err(|err| tracing::debug!(%err, "could not download message media"))
        .ok()?;

    let item = ImageItem::new(photo.file.unique_id.0.clone(), &bytes);
    let scores = app
        .detector
        .classify(std::slice::from_ref(&item))
        .await
        .ok()?;
    scores.into_values().next()
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
fn display_name(user: &User) -> String {
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
