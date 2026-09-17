//! Domain types and their validation (`User`, `Project`, `Session`, `Task`,
//! `Secret`, ...). Models never contain SQL; each carries its own error enum.

// Empty module: the glob import is the crate convention (`CLAUDE.md`, "Backend
// conventions"). The first real file here removes this `allow`.
#![allow(unused_imports)]

use crate::prelude::*;
