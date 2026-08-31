// Execution-provider selection for the RVC ONNX inference engine (ContentVec,
// RMVPE, synthesizer sessions — [E10-S6a]).
//
// `ort`'s `SessionBuilder::with_execution_providers` tries each dispatched
// provider in the given order; a provider that fails to register (missing
// driver, no GPU, EP not compiled in) logs a warning and falls through to
// the next one, with ONNX Runtime's own CPU execution provider as the
// ultimate fallback regardless of what's in the list. `build_providers`
// below just constructs that ordered list — it makes no FFI calls (`.build()`
// is a plain Rust `Arc::new`), so it needs no `onnxruntime` shared library
// loaded to run, unlike `ExecutionProvider::is_available()`/`register()`.
//
// See docs/voice-changer-rvc-pipeline.md's "Rust component architecture"
// section for how this fits into the wider engine.

use ort::ep::{ExecutionProviderDispatch, OpenVINO, ROCm, CPU, CUDA};

/// A hardware acceleration backend the RVC engine can run on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuBackend {
    Cuda,
    Rocm,
    OpenVino,
    Cpu,
}

impl GpuBackend {
    /// Human-readable name, for `GetVCCapabilities`/`DetectGPU`-style UI display.
    pub fn as_str(self) -> &'static str {
        match self {
            GpuBackend::Cuda => "CUDA",
            GpuBackend::Rocm => "ROCm",
            GpuBackend::OpenVino => "OpenVINO",
            GpuBackend::Cpu => "CPU",
        }
    }
}

/// Fixed priority order: GPU backends are tried before falling back to CPU.
/// Matches the architecture decided in `docs/voice-changer-rvc-pipeline.md`.
pub const PRIORITY_ORDER: [GpuBackend; 4] = [
    GpuBackend::Cuda,
    GpuBackend::Rocm,
    GpuBackend::OpenVino,
    GpuBackend::Cpu,
];

/// Build the `ExecutionProviderDispatch` list to pass to
/// `SessionBuilder::with_execution_providers`, preserving `order`. Every
/// entry uses default (untuned) options — per-backend tuning (device index,
/// arena strategy, ...) can be layered on later without changing this
/// function's shape.
pub fn build_providers(order: &[GpuBackend]) -> Vec<ExecutionProviderDispatch> {
    order
        .iter()
        .map(|backend| match backend {
            GpuBackend::Cuda => CUDA::default().build(),
            GpuBackend::Rocm => ROCm::default().build(),
            GpuBackend::OpenVino => OpenVINO::default().build(),
            GpuBackend::Cpu => CPU::default().build(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priority_order_tries_gpu_backends_before_cpu() {
        let idx = |b: GpuBackend| PRIORITY_ORDER.iter().position(|&x| x == b).unwrap();
        assert!(idx(GpuBackend::Cuda) < idx(GpuBackend::Rocm));
        assert!(idx(GpuBackend::Rocm) < idx(GpuBackend::OpenVino));
        assert!(idx(GpuBackend::OpenVino) < idx(GpuBackend::Cpu));
    }

    #[test]
    fn priority_order_ends_with_cpu() {
        assert_eq!(PRIORITY_ORDER.last(), Some(&GpuBackend::Cpu));
    }

    #[test]
    fn build_providers_returns_one_dispatch_per_backend_in_order() {
        assert_eq!(build_providers(&PRIORITY_ORDER).len(), 4);
        assert_eq!(build_providers(&[GpuBackend::Cpu]).len(), 1);
        assert_eq!(build_providers(&[]).len(), 0);
    }

    #[test]
    fn as_str_matches_onnxruntime_backend_names() {
        assert_eq!(GpuBackend::Cuda.as_str(), "CUDA");
        assert_eq!(GpuBackend::Rocm.as_str(), "ROCm");
        assert_eq!(GpuBackend::OpenVino.as_str(), "OpenVINO");
        assert_eq!(GpuBackend::Cpu.as_str(), "CPU");
    }
}
