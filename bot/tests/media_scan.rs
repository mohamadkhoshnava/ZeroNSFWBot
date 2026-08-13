//! The group media scan's own rules.
//!
//! Its whole reason for existing is that it is *not* the profile scan: a
//! different threshold, a different action, and a scope that ignores the grace
//! window. These tests pin down that separation, because the failure mode is
//! silent — wire the two together by accident and a group that set 90% for
//! posted pictures gets 40% instead, which is a wave of deleted holiday photos.

use zeronsfw_bot::{
    config::GroupDefaults,
    db::models::{GroupSettings, MediaKind},
    i18n::Lang,
    policy::{Action, Policy},
    scan::{ImageScoring, Sampled},
};

fn defaults() -> GroupDefaults {
    GroupDefaults {
        lang: Lang::En,
        threshold: 40,
        policy: Policy::NsfwAndContact,
        action: Action::Ban,
        profile_photos_to_scan: 2,
        dry_run: false,
        grace_messages: 5,
        delete_bot_messages: false,
        bot_message_ttl_secs: 60,
        media_scan: false,
        media_threshold: 90,
        media_action: Action::Delete,
        media_frames: 5,
    }
}

fn settings() -> GroupSettings {
    GroupSettings::defaults(-1001, &defaults())
}

/// A group that turned the scan on and left everything else alone.
fn scanning() -> GroupSettings {
    let mut settings = settings();
    settings.media_scan = true;
    settings
}

#[test]
fn the_media_scan_is_off_until_a_group_asks_for_it() {
    let settings = settings();

    assert!(!settings.media_scan);
    for kind in MediaKind::ALL {
        assert!(
            !settings.scans_media_kind(kind),
            "{kind} was scanned without the section being enabled"
        );
    }
}

/// The number the user asked for, and the reason the section exists: a posted
/// picture is convicted on its own, with no bio or avatar to corroborate it, so
/// its bar is far above the profile's.
#[test]
fn media_and_profile_thresholds_are_independent() {
    let mut settings = scanning();

    assert_eq!(settings.threshold, 40);
    assert_eq!(settings.media_threshold, 90);
    assert!((settings.media_threshold_ratio() - 0.9).abs() < f32::EPSILON);

    // Moving one must not move the other.
    settings.threshold = 55;
    assert_eq!(settings.media_threshold, 90);
    settings.media_threshold = 75;
    assert_eq!(settings.threshold, 55);
}

/// A picture that would trigger the profile filter comfortably must still be
/// left alone by the media scan until it clears the higher bar.
#[test]
fn a_score_between_the_two_thresholds_acts_on_neither_by_the_media_rule() {
    let settings = scanning();
    let scoring = ImageScoring::screened(0.62);

    assert!(
        scoring.score >= settings.threshold_ratio(),
        "0.62 is over the profile threshold, as intended for this case"
    );
    assert!(
        scoring.score < settings.media_threshold_ratio(),
        "the media scan must not act at 62% when its bar is 90%"
    );
}

#[test]
fn the_media_action_is_separate_from_the_profile_action() {
    let settings = scanning();

    // Deleting the picture, not banning the member who posted it.
    assert_eq!(settings.media_action, Action::Delete);
    assert_eq!(settings.action, Action::Ban);
}

#[test]
fn every_kind_is_covered_once_the_section_is_on() {
    let settings = scanning();

    for kind in MediaKind::ALL {
        assert!(settings.scans_media_kind(kind), "{kind} was not covered");
    }
}

#[test]
fn a_group_can_drop_the_expensive_kinds_and_keep_the_rest() {
    let mut settings = scanning();
    settings.media_kinds = vec![MediaKind::Photo, MediaKind::Sticker];

    assert!(settings.scans_media_kind(MediaKind::Photo));
    assert!(settings.scans_media_kind(MediaKind::Sticker));
    assert!(!settings.scans_media_kind(MediaKind::Video));
    assert!(!settings.scans_media_kind(MediaKind::Animation));
}

#[test]
fn an_empty_kind_list_scans_nothing() {
    let mut settings = scanning();
    settings.media_kinds = Vec::new();

    for kind in MediaKind::ALL {
        assert!(!settings.scans_media_kind(kind));
    }
}

#[test]
fn media_kinds_round_trip_through_their_stored_names() {
    for kind in MediaKind::ALL {
        assert_eq!(kind.as_str().parse::<MediaKind>().unwrap(), kind);
    }
    // The name a person would reach for, accepted alongside the stored one.
    assert_eq!("gif".parse::<MediaKind>().unwrap(), MediaKind::Animation);
    assert!("voice".parse::<MediaKind>().is_err());
}

/// Frame sampling is the answer to "the explicit part is not at the start".
/// The detail line has to say so, or an admin looking at an innocuous preview
/// in the chat has no way to understand the report.
#[test]
fn a_detection_from_a_clip_names_the_frame_it_came_from() {
    let scoring = ImageScoring {
        fast: 0.91,
        score: 0.94,
        verified: true,
        labels: vec![("porn".into(), 0.93)],
        sampled: Some(Sampled { frame: 7, of: 9 }),
    };

    let detail = scoring.detail();
    assert!(detail.contains("frame 7/9"), "detail was {detail:?}");
    assert!(
        detail.contains("91%") && detail.contains("94%"),
        "both stages must still be shown: {detail:?}"
    );
}

/// A still image has no frames to speak of, and "frame 1/1" on every photo
/// would be noise in every report.
#[test]
fn a_still_image_says_nothing_about_frames() {
    assert_eq!(ImageScoring::screened(0.97).detail(), "97%");
}

/// Sampling defaults to several frames, not one. A default of 1 would ship the
/// old thumbnail-only behaviour under a new name.
#[test]
fn clips_are_sampled_at_more_than_one_frame_by_default() {
    let settings = scanning();

    assert!(settings.media_frames > 1, "a single frame is a guess");
    assert!(settings.media_frames <= zeronsfw_bot::db::models::MAX_MEDIA_FRAMES);
}
