use crate::config::FrameGenBackend;

/// Per-title/series override wins over the global default when present.
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
            resolve_backend(FrameGenBackend::Svp, None),
            FrameGenBackend::Svp
        );
    }
}
