//! HTTP client for the ONNX detector container.
//!
//! Mirrors `detector/app/schemas.py`; changing one side means changing both.

use std::{collections::HashMap, time::Duration};

use anyhow::{Context, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use serde::{Deserialize, Serialize};

/// One image to classify, tagged with a caller-chosen id.
///
/// The bot passes Telegram's `file_unique_id`, which is stable for the lifetime
/// of a photo, so results can be matched up (and cached) without hashing bytes.
#[derive(Debug, Serialize)]
pub struct ImageItem {
    pub id: String,
    pub data: String,
}

impl ImageItem {
    pub fn new(id: impl Into<String>, bytes: &[u8]) -> Self {
        Self {
            id: id.into(),
            data: B64.encode(bytes),
        }
    }
}

#[derive(Debug, Serialize)]
struct ClassifyRequest<'a> {
    images: &'a [ImageItem],
}

#[derive(Debug, Serialize)]
struct OcrRequest<'a> {
    images: &'a [ImageItem],
}

#[derive(Debug, Clone, Deserialize)]
pub struct ClassifyResult {
    pub id: String,
    #[serde(default)]
    pub nsfw: f32,
    #[serde(default)]
    pub sfw: f32,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ClassifyResponse {
    results: Vec<ClassifyResult>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OcrResult {
    pub id: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OcrResponse {
    results: Vec<OcrResult>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Health {
    pub status: String,
    pub model_loaded: bool,
    pub ocr_enabled: bool,
}

#[derive(Clone)]
pub struct DetectorClient {
    http: reqwest::Client,
    base_url: String,
    ocr_enabled: bool,
}

impl DetectorClient {
    pub fn new(base_url: impl Into<String>, timeout: Duration, ocr_enabled: bool) -> Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(timeout)
                // The detector is a single in-network peer; keeping the
                // connection warm removes a TCP handshake per comment.
                .pool_idle_timeout(Duration::from_secs(90))
                .build()
                .context("failed to build the detector HTTP client")?,
            base_url: base_url.into(),
            ocr_enabled,
        })
    }

    pub fn ocr_enabled(&self) -> bool {
        self.ocr_enabled
    }

    /// Score a batch of images. Returns `id -> nsfw probability`.
    ///
    /// Images the service could not decode are simply absent from the map, so
    /// a caller taking a maximum naturally ignores them.
    pub async fn classify(&self, images: &[ImageItem]) -> Result<HashMap<String, f32>> {
        if images.is_empty() {
            return Ok(HashMap::new());
        }

        let response: ClassifyResponse = self
            .http
            .post(format!("{}/classify", self.base_url))
            .json(&ClassifyRequest { images })
            .send()
            .await
            .context("detector /classify request failed")?
            .error_for_status()
            .context("detector /classify returned an error status")?
            .json()
            .await
            .context("detector /classify returned an unexpected body")?;

        Ok(response
            .results
            .into_iter()
            .filter_map(|r| {
                if let Some(err) = &r.error {
                    tracing::debug!(id = %r.id, %err, "detector could not classify an image");
                    return None;
                }
                Some((r.id, r.nsfw))
            })
            .collect())
    }

    /// Read any text burned into the images. Returns `id -> text`.
    ///
    /// OCR is an enrichment, not a gate: transport failures degrade to an empty
    /// map rather than aborting the scan.
    pub async fn ocr(&self, images: &[ImageItem]) -> HashMap<String, String> {
        if !self.ocr_enabled || images.is_empty() {
            return HashMap::new();
        }

        let result = async {
            let response: OcrResponse = self
                .http
                .post(format!("{}/ocr", self.base_url))
                .json(&OcrRequest { images })
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            Ok::<_, reqwest::Error>(response)
        }
        .await;

        match result {
            Ok(response) => response
                .results
                .into_iter()
                .filter(|r| r.error.is_none() && !r.text.trim().is_empty())
                .map(|r| (r.id, r.text))
                .collect(),
            Err(err) => {
                tracing::warn!(%err, "OCR request failed; continuing without avatar text");
                HashMap::new()
            }
        }
    }

    pub async fn health(&self) -> Result<Health> {
        Ok(self
            .http
            .get(format!("{}/health", self.base_url))
            .send()
            .await
            .context("detector /health request failed")?
            .error_for_status()?
            .json()
            .await?)
    }
}
