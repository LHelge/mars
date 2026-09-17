//! The wrapper around the `git` binary, mirror and session clone operations
//! and the `GitCredentialProvider`. Git is never a crate (ADR 0011).

// Empty module: the glob import is the crate convention (`CLAUDE.md`, "Backend
// conventions"). The first real file here removes this `allow`.
#![allow(unused_imports)]

use crate::prelude::*;
