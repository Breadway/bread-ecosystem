//! Execution-provider selection.
//!
//! This crate defaults AMD iGPU acceleration to
//! [`ort::ep::MIGraphX`](ort::ep::MIGraphX), *not*
//! [`ort::ep::ROCm`](ort::ep::ROCm), on purpose. `breadpad-shared/src/
//! classifier.rs::try_load_session` used the classic `ROCMExecutionProvider`
//! and — per the hard-won lesson recorded in this machine's own operator
//! notes (`breadsearch-gpu-backends`, from `breadsearch`'s own history) —
//! that EP silently no-ops on this class of system and falls back to CPU
//! with zero visible error: distro ROCm ONNX Runtime builds (e.g. Arch's
//! `onnxruntime-rocm`) are commonly compiled with `--use_migraphx`, not
//! `--use_rocm`, so `ROCMExecutionProvider` never actually registers, and
//! nothing surfaces that fact unless a `tracing` subscriber is initialized
//! to catch ONNX Runtime's own EP-registration log line. `breadmill/src/
//! embed.rs::rocm_session` already got this right; this module promotes
//! that provider choice (and the loud logging around it) to the shared
//! crate so it can't silently regress in any consumer again.

use std::path::PathBuf;

/// A requested execution provider, in the shared vocabulary consumers use.
/// Convert to an `ort` dispatch entry with [`Provider::to_dispatch`].
#[derive(Debug, Clone)]
pub enum Provider {
    Cpu,
    /// AMD iGPU/dGPU via MIGraphX (ROCm-backed onnxruntime builds). See this
    /// module's doc comment for why this — not `ROCm` — is the correct
    /// choice on this class of system.
    MiGraphX { device_id: i32 },
    /// NVIDIA GPU via CUDA.
    Cuda { device_id: i32 },
    /// Intel iGPU/dGPU (Arc) via OpenVINO. `cache_dir` stores OpenVINO's
    /// compiled-model blobs between runs.
    OpenVino { device_type: String, cache_dir: PathBuf },
    /// AMD XDNA NPU via the VitisAI execution provider (Ryzen AI SDK).
    /// `cache_dir` stores the compiled NPU model between runs.
    Vitis {
        config_file: PathBuf,
        cache_dir: PathBuf,
        cache_key: String,
    },
}

impl Provider {
    pub fn name(&self) -> &'static str {
        match self {
            Provider::Cpu => "CPU",
            Provider::MiGraphX { .. } => "MIGraphX (AMD iGPU/dGPU)",
            Provider::Cuda { .. } => "CUDA (NVIDIA GPU)",
            Provider::OpenVino { .. } => "OpenVINO (Intel iGPU/dGPU)",
            Provider::Vitis { .. } => "VitisAI (AMD XDNA NPU)",
        }
    }

    /// The literal execution-provider name ONNX Runtime's own log line
    /// reports on successful registration (e.g. `"Successfully registered
    /// \`MIGraphXExecutionProvider\`"`) — used to build the loud log hint in
    /// [`crate::session::build_session`].
    fn ort_registration_name(&self) -> &'static str {
        match self {
            Provider::Cpu => "CPUExecutionProvider",
            Provider::MiGraphX { .. } => "MIGraphXExecutionProvider",
            Provider::Cuda { .. } => "CUDAExecutionProvider",
            Provider::OpenVino { .. } => "OpenVINOExecutionProvider",
            Provider::Vitis { .. } => "VitisAIExecutionProvider",
        }
    }

    pub(crate) fn to_dispatch(&self) -> anyhow::Result<ort::ep::ExecutionProviderDispatch> {
        Ok(match self {
            Provider::Cpu => ort::ep::CPU::default().build(),
            Provider::MiGraphX { device_id } => {
                ort::ep::MIGraphX::default().with_device_id(*device_id).build()
            }
            Provider::Cuda { device_id } => {
                ort::ep::CUDA::default().with_device_id(*device_id).build()
            }
            Provider::OpenVino { device_type, cache_dir } => {
                std::fs::create_dir_all(cache_dir)?;
                ort::ep::OpenVINO::default()
                    .with_device_type(device_type.clone())
                    .with_cache_dir(cache_dir.to_string_lossy())
                    .build()
            }
            Provider::Vitis { config_file, cache_dir, cache_key } => {
                std::fs::create_dir_all(cache_dir)?;
                ort::ep::Vitis::default()
                    .with_config_file(config_file.to_string_lossy())
                    .with_cache_dir(cache_dir.to_string_lossy())
                    .with_cache_key(cache_key.clone())
                    .build()
            }
        })
    }

    /// Log a loud, consistent "using X" line plus (for non-CPU providers) a
    /// reminder of exactly what to grep ONNX Runtime's own log output for —
    /// this is the "at minimum log EP registration success/failure loudly
    /// by default" half of the fix, independent of whether the caller has
    /// wired up `tracing_subscriber` (see [`crate::init_tracing`]).
    pub(crate) fn log_selection(&self) {
        tracing::info!("bread-onnx: requesting {} execution provider", self.name());
        if !matches!(self, Provider::Cpu) {
            tracing::info!(
                "bread-onnx: check ONNX Runtime's own log output for \"Successfully registered \
                 `{}`\" — if it's missing, the ONNX Runtime build in use wasn't compiled/shipped \
                 with this provider and inference silently fell back to CPU. This line only \
                 appears if a `tracing` subscriber is initialized (see `bread_onnx::init_tracing`).",
                self.ort_registration_name()
            );
        }
    }
}
