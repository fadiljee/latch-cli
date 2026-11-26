//! Implementasi SandboxProvider (v0.1: NoopProvider; Landlock hadir di Fase 3).

use latch_core::{SandboxCapabilities, SandboxError, SandboxPolicy, SandboxProvider, SpawnSpec};

/// Penyedia sandbox tanpa isolasi untuk v0.1 serta OS non-Linux.
#[derive(Debug, Default)]
pub struct NoopProvider;

impl SandboxProvider for NoopProvider {
    fn name(&self) -> &'static str {
        "noop"
    }

    fn capabilities(&self) -> SandboxCapabilities {
        SandboxCapabilities {
            available: false,
            abi: None,
            detail: "no OS sandbox enabled (v0.1 noop)".to_string(),
        }
    }

    fn wrap_command(
        &self,
        _policy: &SandboxPolicy,
        _spec: &mut SpawnSpec,
    ) -> Result<(), SandboxError> {
        tracing::warn!("sandbox disabled: session is NOT isolated");
        Ok(())
    }
}

/// Mengembalikan penyedia sandbox bawaan untuk host saat ini.
#[must_use]
pub fn default_provider() -> Box<dyn SandboxProvider> {
    Box::new(NoopProvider)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn noop_provider_leaves_spec_unchanged() {
        let provider = default_provider();
        assert_eq!(provider.name(), "noop");
        assert!(!provider.capabilities().available);

        let mut spec = SpawnSpec {
            program: "sh".to_string(),
            args: vec!["-c".to_string(), "true".to_string()],
            env: vec![],
            cwd: PathBuf::from("/tmp"),
        };
        provider
            .wrap_command(&SandboxPolicy::default(), &mut spec)
            .unwrap();
        assert_eq!(spec.program, "sh");
    }
}
