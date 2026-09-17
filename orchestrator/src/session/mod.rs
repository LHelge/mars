//! The `SessionOwner` task, the launcher (including launch-for-task), the
//! idle reaper and startup recovery.

// Empty module: the glob import is the crate convention (`CLAUDE.md`, "Backend
// conventions"). The first real file here removes this `allow`.
#![allow(unused_imports)]

use crate::prelude::*;
