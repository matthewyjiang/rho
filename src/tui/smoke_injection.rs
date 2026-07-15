#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Termination {
    Error,
    Panic,
}

pub(super) fn after_terminal_init() -> anyhow::Result<()> {
    #[cfg(debug_assertions)]
    {
        if std::env::var_os("HERDR_ENV").is_none() {
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

    #[test]
    fn parses_only_explicit_error_and_panic_modes() {
        assert_eq!(parse_termination("error").unwrap(), Termination::Error);
        assert_eq!(parse_termination("panic").unwrap(), Termination::Panic);
        assert_eq!(
            parse_termination("other").unwrap_err().to_string(),
            "unknown RHO_TUI_TEST_TERMINATION value 'other'"
        );
    }
}
