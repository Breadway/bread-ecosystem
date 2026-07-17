//! Shared ONNX Runtime plumbing for the bread ecosystem.
//!
//! Extracted from breadarr, breadsearch, and breadpad during the
//! 2026-07-16 ecosystem-wide utility audit — see each module's doc comment
//! for the original file:line duplication it replaces.
//!
//! **Important**: [`session::build_session`] logs execution-provider
//! selection via the `tracing` crate, but does *not* initialize a
//! subscriber itself. Without one, ONNX Runtime's own "successfully
//! registered `XExecutionProvider`" log line (and this crate's own
//! selection logging) go nowhere — which is exactly how a GPU execution
//! provider can silently no-op back to CPU with zero visible error (see
//! [`provider`]'s doc comment for the concrete history behind this). All
//! three current consumers already call `tracing_subscriber::fmt().init()`
//! (or an `EnvFilter`-configured equivalent) at startup; any new consumer
//! must do the same before calling [`session::build_session`].
//!
//! - [`provider`] — the [`provider::Provider`] enum and the
//!   MIGraphX-not-ROCm default rationale.
//! - [`session`] — session construction with EP fallback + loud logging.
//! - [`embedding`] — the shared tokenize → tensor → mean-pool → normalize
//!   pipeline for BERT-family embedding models.
//! - [`download`] — model download with atomic write + optional SHA-256
//!   integrity check.

pub mod download;
pub mod embedding;
pub mod provider;
pub mod session;

pub use provider::Provider;
pub use session::build_session;
