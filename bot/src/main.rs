use std::time::Duration;

use anyhow::{Context, Result};
use teloxide::{
    Bot,
    adaptors::throttle::Limits,
    prelude::*,
    types::{BotCommand, ParseMode},
    utils::command::BotCommands,
};
use zeronsfw_bot::{App, Tg, config::Config, db, handlers, handlers::commands::Command};

/// How often expired profile-scan entries are swept.
const CACHE_PRUNE_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

#[tokio::main]
async fn main() -> Result<()> {
    // Optional: in Docker the environment comes from compose, not a file.
    let _ = dotenvy::dotenv();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,zeronsfw_bot=debug".into()),
        )
        .with_target(true)
        .init();

    let cfg = Config::from_env().context("invalid configuration")?;

    // Throttle first, so the parse-mode wrapper sits outside it and every call
    // — including the ones inside spawned tasks — is rate limited.
    let bot: Tg = Bot::new(cfg.bot_token.clone())
        .throttle(Limits::default())
        .parse_mode(ParseMode::Html);

    let me = bot.get_me().await.context(
        "could not reach Telegram — check TELEGRAM_BOT_TOKEN and outbound network access",
    )?;
    tracing::info!(username = %me.username(), id = me.id.0, "authenticated");

    let app = App::new(cfg, me.id.0 as i64).await?;

    match app.detector.health().await {
        Ok(health) => tracing::info!(
            model_loaded = health.model_loaded,
            ocr = health.ocr_enabled,
            "detector reachable"
        ),
        // Not fatal: the text-based filters still work, and the detector may
        // still be loading its model.
        Err(err) => tracing::warn!(%err, "detector not reachable at startup"),
    }

    if let Err(err) = bot.set_my_commands(command_menu()).await {
        tracing::warn!(%err, "could not publish the command menu");
    }

    spawn_cache_pruner(app.clone());

    tracing::info!("dispatcher starting");
    Dispatcher::builder(bot, handlers::schema())
        .dependencies(dptree::deps![app])
        .default_handler(|_| async {})
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;

    tracing::info!("shut down cleanly");
    Ok(())
}

/// The command list Telegram shows in the "/" menu.
///
/// `/info` and `/broadcast` are omitted on purpose: they are super-admin only,
/// and advertising them to everyone invites pointless attempts.
fn command_menu() -> Vec<BotCommand> {
    Command::bot_commands()
        .into_iter()
        .filter(|c| !matches!(c.command.as_str(), "/info" | "/broadcast"))
        .collect()
}

/// Periodically drop profile scans old enough that the model may now disagree
/// with them, even though the photo has not changed.
fn spawn_cache_pruner(app: std::sync::Arc<App>) {
    let days = (app.cfg.scan_cache_ttl.as_secs() / 86_400).max(1) as i64;

    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(CACHE_PRUNE_INTERVAL);
        // The first tick fires immediately; skip it so startup stays quiet.
        ticker.tick().await;

        loop {
            ticker.tick().await;
            match db::scan_cache::prune(&app.db, days).await {
                Ok(removed) if removed > 0 => tracing::info!(removed, "pruned scan cache"),
                Ok(_) => {}
                Err(err) => tracing::warn!(%err, "scan cache prune failed"),
            }
        }
    });
}
