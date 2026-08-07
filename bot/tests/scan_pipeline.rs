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
    scan::{PhotoAccess, ScanContext},
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
    }
}

/// A user with nothing suspicious about them.
fn clean_context() -> ScanContext {
    ScanContext {
        user_id: 4242,
        display_name: "Maryam".into(),
        username: Some("maryam".into()),
        bio: Some("Photographer in Tehran".into()),
        photos: PhotoAccess::Visible,
        profile_nsfw: Some(0.02),
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
    ctx.profile_nsfw = Some(0.93);
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
    ctx.profile_nsfw = Some(0.88);

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
    ctx.profile_nsfw = Some(0.77);
    ctx.bio = Some("hey".into());
    ctx.avatar_text = Some("join @my_hot_channel".into());

    let (report, verdict) = run(&ctx).await;

    assert!(report.triggered("profile_ocr"));
    assert!(verdict.matched);
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
    ctx.profile_nsfw = Some(0.95);
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
    ctx.profile_nsfw = Some(0.45);

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
    ctx.profile_nsfw = Some(0.88);

    let (_, verdict) = run(&ctx).await;
    assert!(verdict.matched);
}

#[tokio::test]
async fn explicit_media_in_the_comment_is_caught() {
    let mut ctx = clean_context();
    ctx.settings.policy = Policy::NsfwOnly;
    ctx.message_nsfw = Some(0.97);

    let (report, verdict) = run(&ctx).await;

    assert!(report.triggered("message_media"));
    assert!(verdict.matched);
}

#[tokio::test]
async fn dry_run_detects_but_never_punishes() {
    let mut ctx = clean_context();
    ctx.settings.dry_run = true;
    ctx.profile_nsfw = Some(0.93);
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
    ctx.profile_nsfw = Some(0.99);
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
    ctx.profile_nsfw = Some(0.93);
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
