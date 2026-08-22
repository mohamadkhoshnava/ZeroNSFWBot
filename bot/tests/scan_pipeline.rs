//! End-to-end behaviour of the moderation pipeline: a gathered
//! [`ScanContext`] → every filter → the group's policy → a verdict.
//!
//! No Telegram and no database: the scan layer's job is to fill the context,
//! and everything after that is pure logic. These tests cover the decisions an
//! operator actually cares about — who gets banned, and who does not.

use zeronsfw_bot::{
    config::GroupDefaults,
    db::models::GroupSettings,
    filters::{FilterRegistry, ScanReport},
    i18n::Lang,
    policy::{self, Action, Policy, Verdict},
    scan::{ImageScoring, LinkedChannel, PersonalChannel, PhotoAccess, ScanContext},
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
        ban_foreign_bots: false,
    }
}

/// A user with nothing suspicious about them.
fn clean_context() -> ScanContext {
    ScanContext {
        user_id: 4242,
        display_name: "Maryam".into(),
        username: Some("maryam".into()),
        bio: Some("Photographer in Tehran".into()),
        personal_channel: PersonalChannel::Absent,
        photos: PhotoAccess::Visible,
        profile_nsfw: Some(ImageScoring::screened(0.02)),
        avatar_text: None,
        message_nsfw: None,
        other_group_bans: Some(0),
        settings: GroupSettings::defaults(-1001, &defaults()),
        reputation_min_bans: 3,
    }
}

async fn run(ctx: &ScanContext) -> (ScanReport, Verdict) {
    let report = FilterRegistry::with_defaults().evaluate(ctx).await;
    let verdict = policy::evaluate(
        &report,
        ctx.settings.policy,
        ctx.settings.action,
        &ctx.settings.custom_filters,
        ctx.settings.dry_run,
    );
    (report, verdict)
}

#[tokio::test]
async fn an_ordinary_user_is_left_alone() {
    let (report, verdict) = run(&clean_context()).await;

    assert!(!verdict.matched, "triggered: {:?}", report.triggered_ids());
}

#[tokio::test]
async fn the_classic_spam_profile_is_banned() {
    // NSFW avatar plus a channel to sell — the pattern the bot exists for.
    let mut ctx = clean_context();
    ctx.profile_nsfw = Some(ImageScoring::screened(0.93));
    ctx.bio = Some("18+ videos → t.me/hot_channel".into());

    let (report, verdict) = run(&ctx).await;

    assert!(verdict.matched);
    assert_eq!(verdict.action, Action::Ban);
    assert!(verdict.score > 0.9);
    assert!(report.triggered("profile_nsfw"));
    assert!(report.triggered("bio_link"));
}

#[tokio::test]
async fn a_racy_avatar_without_advertising_is_not_banned() {
    // The single most important false positive to avoid under the default
    // policy: a real person whose avatar the model dislikes.
    let mut ctx = clean_context();
    ctx.profile_nsfw = Some(ImageScoring::screened(0.88));

    let (_, verdict) = run(&ctx).await;
    assert!(!verdict.matched);
}

#[tokio::test]
async fn advertising_without_an_nsfw_avatar_is_not_banned() {
    let mut ctx = clean_context();
    ctx.bio = Some("My work: https://example.com/portfolio".into());

    let (report, verdict) = run(&ctx).await;
    assert!(report.triggered("bio_link"));
    assert!(!verdict.matched, "a link alone must never be enough");
}

#[tokio::test]
async fn contact_info_hidden_on_the_avatar_still_counts_as_advertising() {
    let mut ctx = clean_context();
    ctx.profile_nsfw = Some(ImageScoring::screened(0.77));
    ctx.bio = Some("hey".into());
    ctx.avatar_text = Some("join @my_hot_channel".into());

    let (report, verdict) = run(&ctx).await;

    assert!(report.triggered("profile_ocr"));
    assert!(verdict.matched);
}

/// The gap this filter closes: a spam account with a deliberately clean bio
/// that advertises through the channel pinned to its profile instead. Before
/// `profile_channel` existed the bio filters saw nothing and the default policy
/// let it through.
#[tokio::test]
async fn an_nsfw_profile_advertising_through_its_attached_channel_is_banned() {
    let mut ctx = clean_context();
    ctx.profile_nsfw = Some(ImageScoring::screened(0.91));
    // Nothing to find in the bio — that is the whole point.
    ctx.bio = Some("just here for the memes".into());
    ctx.personal_channel = PersonalChannel::Linked(LinkedChannel {
        title: Some("Hot Videos 18+".into()),
        username: Some("hot_videos_18".into()),
    });

    let (report, verdict) = run(&ctx).await;

    assert!(!report.triggered("bio_link"), "the bio really is clean");
    assert!(report.triggered("profile_channel"));
    assert!(verdict.matched, "the channel is advertising like any link");
    assert_eq!(verdict.action, Action::Ban);
    assert!(
        verdict.reasons.contains(&"profile_channel"),
        "the ban report must name the channel: {:?}",
        verdict.reasons
    );
}

/// A private channel has no @handle, and attaching one is still advertising.
#[tokio::test]
async fn an_attached_private_channel_counts_too() {
    let mut ctx = clean_context();
    ctx.profile_nsfw = Some(ImageScoring::screened(0.91));
    ctx.personal_channel = PersonalChannel::Linked(LinkedChannel {
        title: Some("VIP".into()),
        username: None,
    });

    let (report, verdict) = run(&ctx).await;

    assert!(report.triggered("profile_channel"));
    assert!(verdict.matched);
}

/// The same rule every other signal follows: an attached channel is contact
/// info, not evidence of NSFW content, so it can never convict on its own.
#[tokio::test]
async fn an_attached_channel_alone_is_not_a_ban() {
    let mut ctx = clean_context();
    ctx.personal_channel = PersonalChannel::Linked(LinkedChannel {
        title: Some("My cooking channel".into()),
        username: Some("maryam_cooks".into()),
    });

    let (report, verdict) = run(&ctx).await;

    assert!(report.triggered("profile_channel"));
    assert!(
        !verdict.matched,
        "an ordinary person with their own channel is not a spammer"
    );
}

/// Same discipline as the photo lookup: a `getChat` that failed must not read
/// as "this profile has no channel", nor satisfy the contact half of an AND.
#[tokio::test]
async fn an_unknown_personal_channel_is_not_a_signal() {
    let mut ctx = clean_context();
    ctx.profile_nsfw = Some(ImageScoring::screened(0.95));
    ctx.bio = None;
    ctx.personal_channel = PersonalChannel::Unknown;

    let (report, verdict) = run(&ctx).await;

    let outcome = report.get("profile_channel").expect("filter ran");
    assert!(!outcome.available, "a failed lookup is unknown, not absent");
    assert!(!verdict.matched, "unknown must not satisfy an AND policy");
}

/// Regression for the false positive that banned a real group member at a
/// reported 0% NSFW score. Plenty of ordinary users hide their avatar and keep
/// a link in their bio; that combination is not evidence of anything.
#[tokio::test]
async fn a_hidden_avatar_with_a_link_is_not_a_ban() {
    let mut ctx = clean_context();
    ctx.photos = PhotoAccess::Absent;
    ctx.profile_nsfw = None;
    ctx.bio = Some("t.me/my_blog".into());

    let (report, verdict) = run(&ctx).await;

    // The signal is still reported — an admin can opt into it via a custom
    // policy — but no preset acts on it.
    assert!(report.triggered("no_photo_link"));
    assert!(
        !verdict.matched,
        "banned a user with no NSFW evidence at all"
    );
}

/// The bug underneath that false positive: a failed or skipped
/// getUserProfilePhotos call reported "this user has no photo", so any
/// transient API error looked like a real signal.
#[tokio::test]
async fn a_failed_photo_lookup_is_unknown_not_absent() {
    let mut ctx = clean_context();
    ctx.photos = PhotoAccess::Unknown;
    ctx.profile_nsfw = None;
    ctx.bio = Some("t.me/my_blog".into());

    let (report, verdict) = run(&ctx).await;

    assert!(
        !report.triggered("no_photo_link"),
        "an unknown photo state must never fire the no-photo filter"
    );
    assert!(!verdict.matched);
}

#[tokio::test]
async fn an_unreadable_bio_does_not_count_as_a_clean_bio() {
    let mut ctx = clean_context();
    ctx.profile_nsfw = Some(ImageScoring::screened(0.95));
    ctx.bio = None;

    let (report, verdict) = run(&ctx).await;

    let bio_link = report.get("bio_link").expect("filter ran");
    assert!(
        !bio_link.available,
        "an unreadable bio is unknown, not empty"
    );
    assert!(!verdict.matched, "unknown must not satisfy an AND policy");
}

#[tokio::test]
async fn the_threshold_is_respected() {
    let mut ctx = clean_context();
    ctx.bio = Some("t.me/channel".into());
    ctx.profile_nsfw = Some(ImageScoring::screened(0.45));

    // 40% threshold: 0.45 is over the line.
    let (_, verdict) = run(&ctx).await;
    assert!(verdict.matched);

    // 60% threshold: the same profile now passes.
    ctx.settings.threshold = 60;
    let (_, verdict) = run(&ctx).await;
    assert!(!verdict.matched);
}

#[tokio::test]
async fn nsfw_only_acts_on_the_avatar_alone() {
    let mut ctx = clean_context();
    ctx.settings.policy = Policy::NsfwOnly;
    ctx.profile_nsfw = Some(ImageScoring::screened(0.88));

    let (_, verdict) = run(&ctx).await;
    assert!(verdict.matched);
}

#[tokio::test]
async fn explicit_media_in_the_comment_is_caught() {
    let mut ctx = clean_context();
    ctx.settings.policy = Policy::NsfwOnly;
    ctx.message_nsfw = Some(ImageScoring::screened(0.97));

    let (report, verdict) = run(&ctx).await;

    assert!(report.triggered("message_media"));
    assert!(verdict.matched);
}

#[tokio::test]
async fn dry_run_detects_but_never_punishes() {
    let mut ctx = clean_context();
    ctx.settings.dry_run = true;
    ctx.profile_nsfw = Some(ImageScoring::screened(0.93));
    ctx.bio = Some("t.me/hot".into());

    let (_, verdict) = run(&ctx).await;

    assert!(verdict.matched, "dry run must still detect");
    assert_eq!(verdict.action, Action::Report, "dry run must not ban");
}

#[tokio::test]
async fn the_shared_blocklist_acts_on_reputation_alone() {
    let mut ctx = clean_context();
    ctx.other_group_bans = Some(4);

    let (report, verdict) = run(&ctx).await;

    assert!(report.triggered("reputation"));
    assert!(verdict.matched);
}

#[tokio::test]
async fn reputation_below_the_minimum_does_nothing() {
    let mut ctx = clean_context();
    ctx.other_group_bans = Some(2);

    let (report, verdict) = run(&ctx).await;

    assert!(!report.triggered("reputation"));
    assert!(!verdict.matched);
}

#[tokio::test]
async fn a_group_that_opted_out_ignores_the_shared_blocklist() {
    let mut ctx = clean_context();
    ctx.settings.global_blocklist = false;
    ctx.other_group_bans = Some(9);

    let (report, verdict) = run(&ctx).await;

    assert!(!report.triggered("reputation"));
    assert!(!verdict.matched);
}

#[tokio::test]
async fn custom_policy_requires_every_chosen_filter() {
    let mut ctx = clean_context();
    ctx.settings.policy = Policy::Custom;
    ctx.settings.custom_filters = vec!["profile_nsfw".into(), "bio_keywords".into()];
    ctx.other_group_bans = Some(0);

    // Only one of the two required filters fires.
    ctx.profile_nsfw = Some(ImageScoring::screened(0.99));
    let (_, verdict) = run(&ctx).await;
    assert!(!verdict.matched);

    // Now both do.
    ctx.bio = Some("onlyfans link below".into());
    let (_, verdict) = run(&ctx).await;
    assert!(verdict.matched);
}

#[tokio::test]
async fn keyword_policy_catches_text_only_spam() {
    let mut ctx = clean_context();
    ctx.settings.policy = Policy::NsfwOrKeywords;
    ctx.display_name = "فیلم سوپر 🔞".into();
    ctx.bio = Some("hi".into());

    let (report, verdict) = run(&ctx).await;

    assert!(report.triggered("bio_keywords"));
    assert!(
        verdict.matched,
        "explicit ad copy needs no image to be spam"
    );
}

#[tokio::test]
async fn the_report_lists_the_signals_that_fired() {
    let mut ctx = clean_context();
    ctx.profile_nsfw = Some(ImageScoring::screened(0.93));
    ctx.bio = Some("t.me/hot_channel xxx".into());

    let (_, verdict) = run(&ctx).await;

    // Sorted and deduplicated, so the message text is deterministic.
    let mut sorted = verdict.reasons.clone();
    sorted.sort_unstable();
    assert_eq!(verdict.reasons, sorted);
    assert!(verdict.reasons.contains(&"profile_nsfw"));
    assert!(verdict.reasons.contains(&"bio_link"));
}

#[tokio::test]
async fn cheap_policies_do_not_request_expensive_data() {
    let registry = FilterRegistry::with_defaults();

    let needs = registry.needs_for(&Policy::NsfwOnly.relevant_filters(&[]));
    assert!(needs.profile_photos);
    assert!(
        !needs.avatar_text,
        "nsfw_only must not trigger OCR downloads"
    );
    assert!(!needs.bio, "nsfw_only must not fetch bios");

    let needs = registry.needs_for(&Policy::NsfwAndContact.relevant_filters(&[]));
    assert!(needs.bio && needs.avatar_text && needs.profile_photos);
}

// ---------------------------------------------------------------------------
// Two-stage scoring, as it appears in the detection report.
// ---------------------------------------------------------------------------

/// The verifier clearing an avatar the fast model flagged. This is the anime
/// case: the screening model has no `drawings` class and calls it explicit;
/// the verifier does, and the group's threshold is applied to *its* number.
#[tokio::test]
async fn a_verified_score_below_the_threshold_clears_the_account() {
    let mut ctx = clean_context();
    ctx.bio = Some("t.me/my_art".into());
    ctx.profile_nsfw = Some(ImageScoring {
        fast: 0.88,
        score: 0.07,
        verified: true,
        labels: vec![("drawings".into(), 0.76), ("neutral".into(), 0.14)],
        sampled: None,
    });

    let (report, verdict) = run(&ctx).await;

    assert!(
        !report.triggered("profile_nsfw"),
        "the verifier said it is a drawing"
    );
    assert!(
        !verdict.matched,
        "an anime avatar with a link is not a spam profile"
    );
}

/// The other direction: both stages agree, so the ban stands.
#[tokio::test]
async fn a_verified_score_above_the_threshold_still_bans() {
    let mut ctx = clean_context();
    ctx.bio = Some("t.me/hot_channel".into());
    ctx.profile_nsfw = Some(ImageScoring {
        fast: 0.88,
        score: 0.94,
        verified: true,
        labels: vec![("porn".into(), 0.91), ("sexy".into(), 0.05)],
        sampled: None,
    });

    let (report, verdict) = run(&ctx).await;

    assert!(report.triggered("profile_nsfw"));
    assert!(verdict.matched);
    // The report must show the working, not just the final number.
    let detail = report.get("profile_nsfw").unwrap().detail.clone().unwrap();
    assert!(
        detail.contains("88%") && detail.contains("94%"),
        "detail was {detail:?}"
    );
    assert!(
        detail.contains("porn"),
        "detail should name the class: {detail:?}"
    );
}

#[test]
fn the_detail_shows_both_stages_and_the_leading_class() {
    let scoring = ImageScoring {
        fast: 0.884,
        score: 0.073,
        verified: true,
        labels: vec![("drawings".into(), 0.761), ("neutral".into(), 0.14)],
        sampled: None,
    };

    assert_eq!(scoring.detail(), "88% → 7% · drawings 76%");
}

#[test]
fn an_unverified_detail_shows_one_number_not_a_misleading_arrow() {
    // Below the threshold, so no second stage ran. Rendering "6% → 6%" would
    // imply a confirmation that never happened.
    assert_eq!(ImageScoring::screened(0.061).detail(), "6%");
}

// ---------------------------------------------------------------------------
// Per-group NSFW categories.
// ---------------------------------------------------------------------------

/// The same verifier breakdown, two groups, two correct answers. This is the
/// whole reason categories are per-group rather than baked into the image.
#[test]
fn the_same_breakdown_scores_differently_per_group() {
    let labels = vec![
        ("sexy".to_string(), 0.90),
        ("drawings".to_string(), 0.06),
        ("porn".to_string(), 0.02),
        ("hentai".to_string(), 0.01),
        ("neutral".to_string(), 0.01),
    ];

    // Default: only explicit classes count, so a suggestive photo passes.
    let lenient = ImageScoring::verified(0.94, labels.clone(), &["porn".into(), "hentai".into()]);
    assert!((lenient.score - 0.03).abs() < 1e-5, "got {}", lenient.score);

    // A stricter group counts `sexy` too, and the same image is now explicit.
    let strict = ImageScoring::verified(
        0.94,
        labels.clone(),
        &["porn".into(), "hentai".into(), "sexy".into()],
    );
    assert!((strict.score - 0.93).abs() < 1e-5, "got {}", strict.score);

    // An art community that also bans drawn explicit content but not art.
    let art = ImageScoring::verified(0.94, labels, &["porn".into()]);
    assert!((art.score - 0.02).abs() < 1e-5, "got {}", art.score);
}

#[test]
fn no_categories_means_nothing_is_ever_explicit() {
    // The panel warns about this, but the arithmetic must agree with the
    // warning rather than quietly falling back to some default.
    let scoring = ImageScoring::verified(
        0.99,
        vec![("porn".into(), 0.99), ("neutral".into(), 0.01)],
        &[],
    );
    assert_eq!(scoring.score, 0.0);
}

#[test]
fn category_matching_ignores_case() {
    let scoring = ImageScoring::verified(
        0.9,
        vec![("Porn".into(), 0.8), ("Neutral".into(), 0.2)],
        &["porn".into()],
    );
    assert!((scoring.score - 0.8).abs() < 1e-5);
}

/// End to end: the avatar the spam accounts actually use.
///
/// `sexy 96% / porn 1%` is not a corner case, it is the shape of nearly every
/// profile this bot is pointed at — so it has to be caught out of the box, and
/// a group that disagrees has to be able to say so.
#[tokio::test]
async fn a_suggestive_avatar_counts_by_default_and_can_be_opted_out_of() {
    let breakdown = vec![("sexy".to_string(), 0.96), ("porn".to_string(), 0.01)];

    let mut ctx = clean_context();
    ctx.bio = Some("t.me/my_channel".into());
    ctx.profile_nsfw = Some(ImageScoring::verified(
        0.94,
        breakdown.clone(),
        &ctx.settings.nsfw_categories,
    ));

    let (report, verdict) = run(&ctx).await;
    assert!(report.triggered("profile_nsfw"));
    assert!(
        verdict.matched,
        "the default categories have to include `sexy`"
    );

    ctx.settings.nsfw_categories = vec!["porn".into(), "hentai".into()];
    ctx.profile_nsfw = Some(ImageScoring::verified(
        0.94,
        breakdown,
        &ctx.settings.nsfw_categories,
    ));

    let (_, verdict) = run(&ctx).await;
    assert!(
        !verdict.matched,
        "this group asked for suggestive avatars to be left alone"
    );
}
