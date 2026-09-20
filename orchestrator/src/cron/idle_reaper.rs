//! The idle reaper's slot on [`CronService`] (`ARCHITECTURE.md`, "Background
//! jobs", the idle reaper row).
//!
//! Nothing but the delegation: the job itself is
//! [`crate::session::idle_reaper::reap_idle`], where it sits beside the owner,
//! the registry and the launcher whose rules it applies.

use chrono::{DateTime, Utc};

use super::{CronService, JobReport};
use crate::prelude::*;
use crate::session::idle_reaper::reap_idle;

impl CronService {
    /// Park `running` conversational sessions idle beyond their profile's
    /// timeout, and stop and fail `running` ephemeral ones (`stalled`).
    pub async fn idle_reaper(&self, now: DateTime<Utc>) -> Result<JobReport> {
        reap_idle(&self.state, now).await
    }
}
