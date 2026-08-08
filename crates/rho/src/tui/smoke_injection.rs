#[cfg(any(debug_assertions, test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Termination {
    Error,
    Panic,
}

pub(super) fn matrix_enabled() -> bool {
    cfg!(debug_assertions)
        && std::env::var_os("RHO_TUI_TEST_MODE").as_deref() == Some(std::ffi::OsStr::new("matrix"))
}

/// Version shown in TUI chrome (session header, setup welcome).
///
/// Production always uses the package version. Debug matrix runs may override
/// via `RHO_TUI_DISPLAY_VERSION` so the docs proof plate stays stable across
/// release bumps without post-processing the SVG.
pub(super) fn display_version() -> String {
    resolve_display_version(
        matrix_enabled(),
        std::env::var("RHO_TUI_DISPLAY_VERSION").ok().as_deref(),
        env!("CARGO_PKG_VERSION"),
    )
}

fn resolve_display_version(matrix: bool, override_version: Option<&str>, package: &str) -> String {
    if matrix {
        if let Some(value) = override_version
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return value.to_owned();
        }
    }
    package.to_owned()
}

/// Cached-model providers list rows from the on-disk model cache, which is
/// empty in an isolated matrix HOME. Seed the fixture provider's models so
/// `/model` and internal-agent model pickers have rows without a network
/// refresh.
#[cfg(debug_assertions)]
pub(super) fn seed_matrix_model_cache() {
    if matrix_enabled() {
        use rho_providers::model::provider_models::{
            replace_cached_provider_models_for_tests, ProviderModel,
        };
        // Exactly the fixture model: extra entries would change scripted
        // navigation in scenarios that step through model lists.
        let models = [ProviderModel {
            provider: "openai".into(),
            model: "gpt-5.5".into(),
            display_name: "gpt-5.5".into(),
            context_window: Some(400_000),
            max_output_tokens: Some(128_000),
            reasoning_capabilities: Default::default(),
        }];
        // A failed seed only leaves pickers empty; the scenario assertion
        // reports it, so there is no user to warn here.
        let _ = replace_cached_provider_models_for_tests("openai", &models);
    }
}

pub(super) fn after_terminal_init() -> anyhow::Result<()> {
    #[cfg(debug_assertions)]
    {
        // Allow injection from Herdr smoke tests or the matrix fixture / PTY harness.
        let herdr = std::env::var_os("HERDR_ENV").is_some();
        let matrix = matrix_enabled();
        if !herdr && !matrix {
            return Ok(());
        }
        let Some(value) = std::env::var_os("RHO_TUI_TEST_TERMINATION") else {
            return Ok(());
        };
        let value = value
            .into_string()
            .map_err(|_| anyhow::anyhow!("RHO_TUI_TEST_TERMINATION must be valid UTF-8"))?;
        match parse_termination(&value)? {
            Termination::Error => anyhow::bail!("deterministic injected TUI application error"),
            Termination::Panic => panic!("deterministic injected TUI panic"),
        }
    }

    #[cfg(not(debug_assertions))]
    Ok(())
}

#[cfg(any(debug_assertions, test))]
fn parse_termination(value: &str) -> anyhow::Result<Termination> {
    match value {
        "error" => Ok(Termination::Error),
        "panic" => Ok(Termination::Panic),
        _ => anyhow::bail!("unknown RHO_TUI_TEST_TERMINATION value '{value}'"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn parses_only_explicit_error_and_panic_modes() {
        assert_eq!(parse_termination("error").unwrap(), Termination::Error);
        assert_eq!(parse_termination("panic").unwrap(), Termination::Panic);
        assert_eq!(
            parse_termination("other").unwrap_err().to_string(),
            "unknown RHO_TUI_TEST_TERMINATION value 'other'"
        );
    }

    // Covers: matrix demo may pin a stable header version; production never does.
    // Owner: tui matrix injection
    #[test]
    fn display_version_override_only_applies_in_matrix_mode() {
        let cases = [
            (false, None, "1.2.3", "1.2.3"),
            (false, Some("9.9.9"), "1.2.3", "1.2.3"),
            (true, None, "1.2.3", "1.2.3"),
            (true, Some(""), "1.2.3", "1.2.3"),
            (true, Some("   "), "1.2.3", "1.2.3"),
            (true, Some("1.0.0"), "1.2.3", "1.0.0"),
            (true, Some(" 1.0.0 "), "1.2.3", "1.0.0"),
        ];
        for (matrix, override_version, package, expected) in cases {
            assert_eq!(
                resolve_display_version(matrix, override_version, package),
                expected,
                "matrix={matrix:?} override={override_version:?}"
            );
        }
    }
}
