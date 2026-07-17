//! Session construction with execution-provider fallback.
//!
//! Builds one `ort::session::Session` whose execution-provider dispatch
//! list is exactly `providers` (in order) with an implicit `CPU` appended
//! if the caller didn't already include one — ONNX Runtime tries each
//! listed EP per-node and falls through the list on failure, so this
//! mirrors (and replaces) the identical `.with_execution_providers([primary,
//! CPU])` pattern already proven out in `breadmill/src/embed.rs::rocm_session`
//! /`cuda_session`/`openvino_session`/`npu_session`.

use std::path::Path;

use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;

use crate::provider::Provider;

/// Build a session, trying each of `providers` in order (ONNX Runtime falls
/// through per-node on registration failure) with a trailing `CPU` fallback
/// implicitly appended if not already present. Always logs which provider
/// was requested — see [`Provider::log_selection`] — regardless of whether
/// `tracing_subscriber` is initialized, so at minimum the *attempt* is
/// visible even without wired-up logging; the actual per-EP success/failure
/// detail only surfaces once a subscriber is listening.
pub fn build_session(
    model_path: &Path,
    opt_level: GraphOptimizationLevel,
    providers: &[Provider],
) -> anyhow::Result<Session> {
    let mut dispatch = Vec::with_capacity(providers.len() + 1);
    for p in providers {
        p.log_selection();
        dispatch.push(p.to_dispatch()?);
    }
    if !providers.iter().any(|p| matches!(p, Provider::Cpu)) {
        dispatch.push(Provider::Cpu.to_dispatch()?);
    }

    let mut builder = Session::builder()
        .map_err(|e| anyhow::anyhow!("failed to create ort session builder: {e}"))?
        .with_optimization_level(opt_level)
        .map_err(|e| anyhow::anyhow!("failed to set optimization level: {e}"))?
        .with_execution_providers(dispatch)
        .map_err(|e| anyhow::anyhow!("failed to configure execution providers: {e}"))?;

    builder
        .commit_from_file(model_path)
        .map_err(|e| anyhow::anyhow!("failed to load model from {}: {e}", model_path.display()))
}
