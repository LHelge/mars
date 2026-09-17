//! The `AgentBackend` trait, the `claude/` adapter and native-output
//! translation into `AgentEvent`s.

// Empty module: the glob import is the crate convention (`CLAUDE.md`, "Backend
// conventions"). The first real file here removes this `allow`.
#![allow(unused_imports)]

use crate::prelude::*;
