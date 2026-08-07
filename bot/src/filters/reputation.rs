use async_trait::async_trait;

use super::{F_REPUTATION, Filter, FilterOutcome, Needs};
use crate::scan::ScanContext;

/// Fires when the account has already been banned for NSFW advertising in
/// enough *other* groups.
///
/// Spam accounts work a list of groups, so the second and third group they hit
/// can benefit from the first group's judgement. The count deliberately
/// excludes the current group: a user already banned here would not be posting,
/// and self-reinforcement would make a single false positive permanent.
///
/// Each group opts in and out of this via `/nsfw → Shared blocklist`.
pub struct Reputation;

#[async_trait]
impl Filter for Reputation {
    fn id(&self) -> &'static str {
        F_REPUTATION
    }

    fn needs(&self) -> Needs {
        Needs {
            reputation: true,
            ..Needs::default()
        }
    }

    async fn evaluate(&self, ctx: &ScanContext) -> FilterOutcome {
        if !ctx.settings.global_blocklist {
            return FilterOutcome::unavailable();
        }

        let Some(count) = ctx.other_group_bans else {
            return FilterOutcome::unavailable();
        };

        if count < ctx.reputation_min_bans {
            return FilterOutcome {
                triggered: false,
                score: 0.0,
                detail: None,
                available: true,
            };
        }

        FilterOutcome::triggered(1.0, Some(count.to_string()))
    }
}
