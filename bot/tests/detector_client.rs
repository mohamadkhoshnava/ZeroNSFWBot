//! Detector client behaviour against a mocked HTTP service.
//!
//! The contract that matters is degradation: the classifier's failures must be
//! loud (`Err`, so the caller can log and skip), while OCR's must be silent
//! (empty map, because avatar text is an enrichment and losing it should never
//! block moderation).

use std::time::Duration;

use serde_json::json;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json_string, method, path},
};
use zeronsfw_bot::detector::{DetectorClient, ImageItem};

fn client(server: &MockServer, ocr: bool) -> DetectorClient {
    DetectorClient::new(server.uri(), Duration::from_secs(5), ocr).expect("client builds")
}

fn image(id: &str) -> ImageItem {
    ImageItem::new(id, b"not-really-a-jpeg")
}

#[tokio::test]
async fn classify_maps_scores_by_image_id() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/classify"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model_id": "test",
            "results": [
                {"id": "a", "nsfw": 0.91, "sfw": 0.09, "error": null},
                {"id": "b", "nsfw": 0.02, "sfw": 0.98, "error": null},
            ]
        })))
        .mount(&server)
        .await;

    let scores = client(&server, false)
        .classify(&[image("a"), image("b")])
        .await
        .expect("classify succeeds");

    assert_eq!(scores.len(), 2);
    assert!((scores["a"] - 0.91).abs() < 1e-6);
    assert!((scores["b"] - 0.02).abs() < 1e-6);
}

#[tokio::test]
async fn images_the_service_could_not_decode_are_dropped() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/classify"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model_id": "test",
            "results": [
                {"id": "good", "nsfw": 0.7, "sfw": 0.3, "error": null},
                {"id": "bad", "nsfw": 0.0, "sfw": 0.0, "error": "decode failed"},
            ]
        })))
        .mount(&server)
        .await;

    let scores = client(&server, false)
        .classify(&[image("good"), image("bad")])
        .await
        .unwrap();

    // A 0.0 for "bad" would drag a max() down and read as "definitely safe".
    assert_eq!(scores.len(), 1);
    assert!(scores.contains_key("good"));
    assert!(!scores.contains_key("bad"));
}

#[tokio::test]
async fn an_empty_batch_makes_no_request() {
    // No mocks are mounted, so any outgoing request fails the test.
    let server = MockServer::start().await;

    let scores = client(&server, true).classify(&[]).await.unwrap();
    assert!(scores.is_empty());
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn classifier_errors_surface_to_the_caller() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/classify"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;

    let result = client(&server, false).classify(&[image("a")]).await;
    assert!(
        result.is_err(),
        "a failing detector must not look like a clean score"
    );
}

#[tokio::test]
async fn ocr_failures_degrade_to_no_text() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/ocr"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let text = client(&server, true).ocr(&[image("a")]).await;
    assert!(text.is_empty());
}

#[tokio::test]
async fn ocr_is_skipped_entirely_when_disabled() {
    let server = MockServer::start().await;

    let text = client(&server, false).ocr(&[image("a")]).await;
    assert!(text.is_empty());
    assert!(
        server.received_requests().await.unwrap().is_empty(),
        "ENABLE_OCR=false must not cost a round trip"
    );
}

#[tokio::test]
async fn ocr_drops_blank_results() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/ocr"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "enabled": true,
            "results": [
                {"id": "a", "text": "t.me/spam", "error": null},
                {"id": "b", "text": "   ", "error": null},
            ]
        })))
        .mount(&server)
        .await;

    let text = client(&server, true).ocr(&[image("a"), image("b")]).await;
    assert_eq!(text.len(), 1);
    assert_eq!(text["a"], "t.me/spam");
}

#[tokio::test]
async fn images_are_sent_base64_encoded_under_their_id() {
    let server = MockServer::start().await;
    let expected = json!({
        "images": [{"id": "abc", "data": "bm90LXJlYWxseS1hLWpwZWc="}]
    })
    .to_string();

    Mock::given(method("POST"))
        .and(path("/classify"))
        .and(body_json_string(expected))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model_id": "test",
            "results": [{"id": "abc", "nsfw": 0.5, "sfw": 0.5, "error": null}]
        })))
        .expect(1)
        .mount(&server)
        .await;

    client(&server, false)
        .classify(&[image("abc")])
        .await
        .expect("the mock only matches the exact wire format");
}

#[tokio::test]
async fn health_reports_model_state() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/health"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": "ok", "model_loaded": true, "ocr_enabled": false
        })))
        .mount(&server)
        .await;

    let health = client(&server, true).health().await.unwrap();
    assert!(health.model_loaded);
    assert_eq!(health.status, "ok");
}

// ---------------------------------------------------------------------------
// Second-stage verification.
//
// The contract is asymmetric on purpose: a missing or broken verifier must be
// reported as `Unavailable`, never as a pass-through of the fast score. The
// fast model is the one that over-flags anime, so silently trusting it when the
// second opinion is missing would reinstate exactly the bans this prevents.
// ---------------------------------------------------------------------------

use zeronsfw_bot::detector::Verification;

#[tokio::test]
async fn verify_returns_the_heavier_models_scores_and_labels() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/verify"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model_id": "verifier",
            "available": true,
            "results": [{
                "id": "avatar", "nsfw": 0.04, "sfw": 0.96, "error": null,
                "labels": {"drawings": 0.91, "hentai": 0.03, "neutral": 0.04,
                           "porn": 0.01, "sexy": 0.01}
            }]
        })))
        .mount(&server)
        .await;

    match client(&server, false).verify(&[image("avatar")]).await {
        Verification::Checked { scores, labels } => {
            // The fast model would have called this explicit; the verifier says
            // it is a drawing.
            assert!((scores["avatar"] - 0.04).abs() < 1e-6);
            assert!((labels["avatar"]["drawings"] - 0.91).abs() < 1e-6);
        }
        Verification::Unavailable => panic!("a healthy verifier must not report unavailable"),
    }
}

#[tokio::test]
async fn a_detector_without_a_verifier_reports_unavailable() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/verify"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model_id": "", "available": false, "results": []
        })))
        .mount(&server)
        .await;

    assert!(matches!(
        client(&server, false).verify(&[image("a")]).await,
        Verification::Unavailable
    ));
}

#[tokio::test]
async fn a_failing_verifier_reports_unavailable_rather_than_guessing() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/verify"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    assert!(matches!(
        client(&server, false).verify(&[image("a")]).await,
        Verification::Unavailable
    ));
}

#[tokio::test]
async fn verifying_nothing_makes_no_request() {
    let server = MockServer::start().await;

    assert!(matches!(
        client(&server, false).verify(&[]).await,
        Verification::Checked { .. }
    ));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn images_the_verifier_could_not_score_are_dropped() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/verify"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model_id": "verifier",
            "available": true,
            "results": [
                {"id": "good", "nsfw": 0.8, "sfw": 0.2, "labels": {}, "error": null},
                {"id": "bad", "nsfw": 0.0, "sfw": 0.0, "labels": {}, "error": "decode failed"}
            ]
        })))
        .mount(&server)
        .await;

    match client(&server, false)
        .verify(&[image("good"), image("bad")])
        .await
    {
        Verification::Checked { scores, .. } => {
            // A 0.0 for "bad" would drag a max() down and read as "confirmed safe".
            assert_eq!(scores.len(), 1);
            assert!(scores.contains_key("good"));
        }
        Verification::Unavailable => panic!("one bad image must not void the batch"),
    }
}

// ---------------------------------------------------------------------------
// Frame sampling.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn frames_come_back_ready_to_classify() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/frames"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "video_enabled": true,
            "results": [{
                "id": "clip",
                "total": 40,
                "decoder": "ffmpeg",
                "error": null,
                "frames": [
                    {"id": "clip#0", "index": 0, "data": "AAAA"},
                    {"id": "clip#20", "index": 20, "data": "BBBB"},
                    {"id": "clip#39", "index": 39, "data": "CCCC"},
                ],
            }]
        })))
        .mount(&server)
        .await;

    let sampled = client(&server, false)
        .frames(&image("clip"), 3)
        .await
        .expect("request succeeds")
        .expect("the clip was decodable");

    assert_eq!(sampled.images.len(), 3);
    assert_eq!(sampled.total, 40);
    // Ids trace each frame back to its clip, so a score can name the frame.
    assert_eq!(sampled.images[1].id, "clip#20");
    // The base64 is passed straight through: decoding it here only to encode
    // it again for /classify would be pure waste.
    assert_eq!(sampled.images[1].data, "BBBB");
}

/// A clip this build cannot decode is a real answer, not a transport failure:
/// the caller falls back to Telegram's thumbnail rather than skipping the
/// check or retrying.
#[tokio::test]
async fn an_undecodable_clip_is_reported_as_no_frames_not_an_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/frames"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "video_enabled": false,
            "results": [{
                "id": "clip",
                "total": 0,
                "decoder": "",
                "frames": [],
                "error": "video decoding is unavailable in this build",
            }]
        })))
        .mount(&server)
        .await;

    let sampled = client(&server, false)
        .frames(&image("clip"), 5)
        .await
        .expect("the request itself succeeded");

    assert!(sampled.is_none());
}

#[tokio::test]
async fn a_frames_failure_is_loud_so_the_caller_can_fall_back() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/frames"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    assert!(
        client(&server, false)
            .frames(&image("clip"), 5)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn the_requested_frame_count_reaches_the_detector() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/frames"))
        .and(body_json_string(
            json!({"images": [{"id": "clip", "data": "bm90LXJlYWxseS1hLWpwZWc="}], "max_frames": 9})
                .to_string(),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "video_enabled": true,
            "results": [{"id": "clip", "total": 1, "decoder": "still", "error": null,
                         "frames": [{"id": "clip#0", "index": 0, "data": "AAAA"}]}]
        })))
        .mount(&server)
        .await;

    assert!(
        client(&server, false)
            .frames(&image("clip"), 9)
            .await
            .expect("request succeeds")
            .is_some()
    );
}
