//! Scanning the media posted in a group, as a subject in its own right.
//!
//! This is deliberately not the `message_media` filter. That one asks "is this
//! account a spam profile?", and the picture in the comment is one clue among
//! the bio, the avatar and the username. It runs inside the profile scan, so
//! the grace window and the clean-user cache apply to it, and it shares the
//! profile threshold.
//!
//! This asks a different question — "is this picture pornography?" — about
//! everyone, forever, and with nothing to corroborate it. Two consequences
//! follow, and they are the whole design:
//!
//! * **Its own threshold.** 40% is a sensible bar for one signal among four.
//!   It is a reckless bar for deleting a long-standing member's photo on the
//!   strength of a model's opinion alone, which is why this defaults to 90.
//! * **Its own action.** Removing an explicit picture is proportionate;
//!   banning the member who posted it is a separate decision, so it is a
//!   separate setting.
//!
//! Clips are sampled rather than thumbnailed. Telegram's poster thumbnail is
//! one arbitrary frame, and a GIF that opens on a cat and ends on pornography
//! is a spam technique, not a hypothetical.

use std::sync::Arc;

use teloxide::{
    net::Download,
    prelude::*,
    types::{FileId, FileMeta, Message, PhotoSize},
};

use crate::{
    App, Tg,
    db::models::{GroupSettings, MediaKind},
    detector::ImageItem,
    scan::{self, ImageScoring},
};

/// The attachment on a message, reduced to what a scan needs.
#[derive(Debug, Clone)]
pub struct Attachment {
    pub kind: MediaKind,
    /// The whole file, when it is a clip small enough to fetch and sample.
    clip: Option<FileRef>,
    /// A single still: the photo itself, or a clip's poster thumbnail. Kept
    /// even when `clip` is set, as the fallback for a clip that cannot be
    /// decoded.
    still: Option<FileRef>,
}

#[derive(Debug, Clone)]
struct FileRef {
    id: FileId,
    unique_id: String,
}

impl From<&FileMeta> for FileRef {
    fn from(meta: &FileMeta) -> Self {
        Self {
            id: meta.id.clone(),
            unique_id: meta.unique_id.0.clone(),
        }
    }
}

impl Attachment {
    /// Stable identity of what will actually be scanned — the cache key.
    ///
    /// Telegram's `file_unique_id` is the same for everyone forever, so a
    /// sticker posted a thousand times is scored once.
    fn key(&self) -> Option<&str> {
        self.clip
            .as_ref()
            .or(self.still.as_ref())
            .map(|f| f.unique_id.as_str())
    }
}

/// What this message carries, if the group scans that kind.
///
/// Returns `None` for anything unscannable or switched off, which is what lets
/// the caller skip the whole pass — including the admin lookup — for the
/// overwhelming majority of messages, which are text.
pub fn find(message: &Message, settings: &GroupSettings, max_bytes: u32) -> Option<Attachment> {
    // A clip is only worth fetching whole if it will fit; past the cap the
    // thumbnail is all there is, which is still better than skipping it.
    let clip = |meta: &FileMeta| (meta.size <= max_bytes).then(|| FileRef::from(meta));
    let still = |photo: &Option<PhotoSize>| photo.as_ref().map(|p| FileRef::from(&p.file));

    if let Some(sizes) = message.photo() {
        let largest = sizes.iter().max_by_key(|p| p.width * p.height)?;
        return scannable(
            settings,
            MediaKind::Photo,
            None,
            Some(FileRef::from(&largest.file)),
        );
    }

    if let Some(animation) = message.animation() {
        return scannable(
            settings,
            MediaKind::Animation,
            clip(&animation.file),
            still(&animation.thumbnail),
        );
    }

    if let Some(video) = message.video() {
        return scannable(
            settings,
            MediaKind::Video,
            clip(&video.file),
            still(&video.thumbnail),
        );
    }

    if let Some(sticker) = message.sticker() {
        // A .tgs sticker is Lottie JSON — vector instructions, not frames.
        // Nothing here can render it, so its thumbnail is the honest best.
        let clip = (!sticker.is_animated())
            .then(|| clip(&sticker.file))
            .flatten();
        // A static sticker *is* an image, so it is its own still and beats the
        // smaller thumbnail.
        let still = if sticker.is_static() {
            Some(FileRef::from(&sticker.file))
        } else {
            still(&sticker.thumbnail)
        };
        return scannable(settings, MediaKind::Sticker, clip, still);
    }

    // GIFs sent as files rather than as animations. Telegram only calls it an
    // `animation` when the client uploaded it as one; the same bytes attached
    // as a document arrive here instead, and they are the same problem.
    if let Some(document) = message.document() {
        let mime = document.mime_type.as_ref()?;
        let is_media = matches!(mime.type_().as_str(), "image" | "video");
        if !is_media {
            return None;
        }
        let kind = if mime.type_() == "video" || mime.subtype() == "gif" {
            MediaKind::Animation
        } else {
            MediaKind::Photo
        };
        return scannable(
            settings,
            kind,
            clip(&document.file),
            still(&document.thumbnail),
        );
    }

    None
}

fn scannable(
    settings: &GroupSettings,
    kind: MediaKind,
    clip: Option<FileRef>,
    still: Option<FileRef>,
) -> Option<Attachment> {
    if !settings.scans_media_kind(kind) {
        return None;
    }
    let attachment = Attachment { kind, clip, still };
    attachment.key()?;
    Some(attachment)
}

/// Score one attachment.
///
/// `verify_at` is the score above which the second stage runs. It is a
/// parameter rather than this group's media threshold because the profile scan
/// may also want this number at a *lower* bar: verifying once at the lower of
/// the two keeps the invariant that no score anything acts on is ever an
/// unverified one.
///
/// `None` means no usable signal — the download failed, the detector is down,
/// or the image was flagged but could not be verified. In every one of those
/// cases the correct behaviour is to do nothing, never to guess.
pub async fn scan(
    app: &Arc<App>,
    bot: &Tg,
    settings: &GroupSettings,
    attachment: &Attachment,
    verify_at: f32,
    user_id: i64,
) -> Option<ImageScoring> {
    let key = attachment.key()?;

    // The same sticker, GIF and meme circulate endlessly in a busy group, and
    // `file_unique_id` never changes, so this is the difference between scoring
    // a popular sticker once and scoring it on every post.
    if let Some(cached) = app.media_scores.get(&key.to_owned()).await
        && reusable(&cached, verify_at)
    {
        return Some(cached);
    }

    let scoring = score(app, bot, settings, attachment, verify_at, user_id).await?;
    app.media_scores
        .insert(key.to_owned(), scoring.clone())
        .await;
    Some(scoring)
}

/// Whether a cached score can answer this group's question.
///
/// A verified score carries the verifier's full breakdown, so any group can
/// derive its own number from it. An *unverified* one is only the screening
/// model's opinion, kept because it fell below the threshold that was in force
/// when it was cached — a group with a lower bar would have escalated it, so
/// for that group this is a miss, not an answer.
fn reusable(cached: &ImageScoring, verify_at: f32) -> bool {
    cached.verified || cached.fast < verify_at
}

async fn score(
    app: &Arc<App>,
    bot: &Tg,
    settings: &GroupSettings,
    attachment: &Attachment,
    verify_at: f32,
    user_id: i64,
) -> Option<ImageScoring> {
    let categories = &settings.nsfw_categories;
    let frames = frames(app, bot, settings, attachment).await?;

    let fast = app.detector.classify(&frames.images).await.ok()?;
    // The worst frame decides. A clip is explicit if *any* part of it is.
    let worst = fast.values().copied().fold(f32::NEG_INFINITY, f32::max);
    let worst = (worst > f32::NEG_INFINITY).then_some(worst)?;

    let confirmed = scan::confirm(
        app,
        &frames.images,
        Some(worst),
        verify_at,
        categories,
        "group_media",
        user_id,
    )
    .await?;

    // Name the frame that lost, but only when there was more than one and the
    // verifier actually picked one. On a still, "frame 1/1" is noise.
    let scoring = match confirmed.image_id.as_deref() {
        Some(id) if frames.images.len() > 1 => {
            let position = frames.images.iter().position(|image| image.id == id);
            match position {
                Some(index) => scoring_from_frame(confirmed.scoring, index, &frames),
                None => confirmed.scoring,
            }
        }
        _ => confirmed.scoring,
    };

    Some(scoring)
}

fn scoring_from_frame(scoring: ImageScoring, index: usize, frames: &Sampled) -> ImageScoring {
    scoring.from_frame(index as u32 + 1, frames.images.len() as u32)
}

/// The images to score: the frames of a clip, or the one still.
struct Sampled {
    images: Vec<ImageItem>,
}

async fn frames(
    app: &Arc<App>,
    bot: &Tg,
    settings: &GroupSettings,
    attachment: &Attachment,
) -> Option<Sampled> {
    if let Some(clip) = &attachment.clip {
        match download(bot, &clip.id).await {
            Ok(bytes) => {
                let media = ImageItem::new(clip.unique_id.clone(), &bytes);
                let wanted = settings.media_frames.max(1) as u32;

                match app.detector.frames(&media, wanted).await {
                    Ok(Some(sampled)) => {
                        tracing::debug!(
                            kind = %attachment.kind,
                            frames = sampled.images.len(),
                            source_frames = sampled.total,
                            "sampled a clip"
                        );
                        return Some(Sampled {
                            images: sampled.images,
                        });
                    }
                    // The detector answered and could not decode it — a video
                    // on a build without ffmpeg, most likely. The thumbnail is
                    // a worse check but an honest one, so fall through to it.
                    Ok(None) => tracing::debug!(
                        kind = %attachment.kind,
                        "clip could not be sampled; falling back to the thumbnail"
                    ),
                    Err(err) => tracing::warn!(%err, "frame extraction failed"),
                }
            }
            Err(err) => tracing::debug!(%err, "could not download the clip"),
        }
    }

    let still = attachment.still.as_ref()?;
    let bytes = download(bot, &still.id)
        .await
        .inspect_err(|err| tracing::debug!(%err, "could not download the media still"))
        .ok()?;

    Some(Sampled {
        images: vec![ImageItem::new(still.unique_id.clone(), &bytes)],
    })
}

async fn download(bot: &Tg, file_id: &FileId) -> anyhow::Result<Vec<u8>> {
    let file = bot.get_file(file_id.clone()).await?;
    let mut buf = Vec::with_capacity(file.size as usize);
    bot.download_file(&file.path, &mut buf).await?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::Sampled as Frame;

    fn scoring(fast: f32, verified: bool) -> ImageScoring {
        ImageScoring {
            fast,
            score: fast,
            verified,
            labels: Vec::new(),
            sampled: None,
        }
    }

    #[test]
    fn a_verified_score_is_reusable_at_any_threshold() {
        let cached = scoring(0.95, true);
        assert!(reusable(&cached, 0.4));
        assert!(reusable(&cached, 0.99));
    }

    /// The trap this avoids: a clip screened at 0.7 and cached unverified,
    /// then reused by a group whose bar is 0.5. That group would be acting on
    /// the screening model's opinion alone — the exact false positive the
    /// second stage exists to prevent.
    #[test]
    fn an_unverified_score_is_only_reusable_below_the_bar_that_cached_it() {
        let cached = scoring(0.7, false);
        assert!(reusable(&cached, 0.9), "0.7 would not escalate at 0.9");
        assert!(!reusable(&cached, 0.5), "0.7 would escalate at 0.5");
    }

    #[test]
    fn the_detail_names_the_frame_that_triggered() {
        let scoring = ImageScoring {
            fast: 0.91,
            score: 0.94,
            verified: true,
            labels: vec![("porn".into(), 0.93)],
            sampled: Some(Frame { frame: 4, of: 5 }),
        };

        let detail = scoring.detail();
        assert!(detail.contains("frame 4/5"), "detail was {detail:?}");
        assert!(detail.contains("91%") && detail.contains("94%"));
    }
}
