//! Data access. One module per concern, all sharing a single [`sqlx::PgPool`].

pub mod appeals;
pub mod audit;
pub mod bans;
pub mod detections;
pub mod groups;
pub mod models;
pub mod scan_cache;
pub mod stats;
pub mod users;

use std::time::Duration;

use anyhow::{Context, Result};
use sqlx::postgres::{PgPool, PgPoolOptions};

/// Connect and run migrations.
///
/// Retries the initial connection: under `docker compose` the bot can win the
/// race against Postgres even with a healthcheck, and crash-looping there is
/// noisy for no reason.
pub async fn connect(url: &str, max_connections: u32) -> Result<PgPool> {
    const ATTEMPTS: u32 = 10;

    let mut last_err = None;
    for attempt in 1..=ATTEMPTS {
        match PgPoolOptions::new()
            .max_connections(max_connections)
            .acquire_timeout(Duration::from_secs(10))
            .connect(url)
            .await
        {
            Ok(pool) => {
                sqlx::migrate!("./migrations")
                    .run(&pool)
                    .await
                    .context("database migrations failed")?;
                tracing::info!("database ready");
                return Ok(pool);
            }
            Err(err) => {
                tracing::warn!(attempt, %err, "database not reachable yet, retrying");
                last_err = Some(err);
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }

    Err(last_err.expect("loop runs at least once")).context("could not connect to the database")
}
