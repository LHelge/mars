//! The production [`ContainerEngine`](super::ContainerEngine) implementation,
//! on `bollard` against the Docker-compatible API (ADR 0004).
//!
//! Empty until the task that writes it. It is the only module in the crate
//! that may name a `bollard` type: everything else speaks the plain types in
//! [`super::types`].
//!
//! Note that this module shadows the `bollard` crate name inside [`super`].
//! This module and its siblings still reach the crate as `bollard::…`, because
//! a `use` path resolves through the extern prelude, but `engine/mod.rs`
//! itself would have to write `::bollard::…`.
