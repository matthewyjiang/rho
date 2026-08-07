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
