//! Shared helpers for the integration tests (`CLAUDE.md`, "Testing
//! expectations").
//!
//! Every test binary declares `mod common;` and uses the subset it needs, so
//! items no single binary touches are expected here rather than a lint to fix.

#![allow(dead_code)]

pub mod db;
