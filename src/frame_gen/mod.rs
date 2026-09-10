pub mod lsfg_vk;
pub mod svp;

use crate::config::FrameGenBackend;

/// Per-title/series override wins over the global default when present.
/// HDR/PGS-subtitle fallback (M11) isn't wired in yet — this is purely
/// the config-precedence resolution for now.
pub fn resolve_backend(
    global_default: FrameGenBackend,
    title_override: Option<FrameGenBackend>,
) -> FrameGenBackend {
    title_override.unwrap_or(global_default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_wins_when_present() {
        assert_eq!(
            resolve_backend(FrameGenBackend::Off, Some(FrameGenBackend::Svp)),
            FrameGenBackend::Svp
        );
    }

    #[test]
    fn falls_back_to_global_default() {
        assert_eq!(
            resolve_backend(FrameGenBackend::LsfgVk, None),
            FrameGenBackend::LsfgVk
        );
    }
}
