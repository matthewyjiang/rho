use pretty_assertions::assert_eq;

use super::*;

// Covers: `GEMINI_HOME` wins over `~/.gemini` (an empty value does not) and
// a leading `~` means the home directory, matching the server.
// Owner: Antigravity home resolution; callers inject the env values.
#[test]
fn gemini_home_override_wins_when_nonempty() {
    let home = Path::new("/home/user");
    for (gemini_home, expected) in [
        (None, "/home/user/.gemini/antigravity-acp"),
        (Some(""), "/home/user/.gemini/antigravity-acp"),
        (Some("/srv/gemini"), "/srv/gemini/antigravity-acp"),
        (Some("~/gemini"), "/home/user/gemini/antigravity-acp"),
        (Some("~"), "/home/user/antigravity-acp"),
    ] {
        assert_eq!(
            AntigravityHome::resolve(gemini_home.map(PathBuf::from), home, TokenStore::File)
                .acp_dir(),
            Path::new(expected),
            "{gemini_home:?}"
        );
    }
}

// Covers: a run is ready only when settings name a method and an OAuth
// method's token file exists (unless the token may live in the macOS
// Keychain); env-based methods need no file; damaged settings report why
// instead of looking signed out.
// Owner: Antigravity sign-in preflight (what keeps runs off the browser flow).
#[test]
fn auth_status_reads_method_and_token_presence() {
    let settings = |method: &str| format!(r#"{{"auth":{{"type":"{method}"}}}}"#);
    /// Name, settings.json contents, token store, token files present,
    /// expected status.
    type Case = (
        &'static str,
        Option<String>,
        TokenStore,
        &'static [&'static str],
        fn(&Path) -> AntigravityAuthStatus,
    );
    let file = TokenStore::File;
    let cases: [Case; 7] = [
        ("no settings", None, file, &[], |dir| {
            AntigravityAuthStatus::SignedOut {
                settings: dir.join("settings.json"),
            }
        }),
        ("no auth type", Some("{}".into()), file, &[], |dir| {
            AntigravityAuthStatus::SignedOut {
                settings: dir.join("settings.json"),
            }
        }),
        (
            "personal without token",
            Some(settings("oauth-personal")),
            file,
            &["acp_business_token.json"],
            |dir| AntigravityAuthStatus::MissingToken {
                method: "oauth-personal".into(),
                token: dir.join("acp_token.json"),
            },
        ),
        (
            "personal with token",
            Some(settings("oauth-personal")),
            file,
            &["acp_token.json"],
            |_| AntigravityAuthStatus::Configured {
                method: "oauth-personal".into(),
            },
        ),
        (
            "business with token",
            Some(settings("oauth-business")),
            file,
            &["acp_business_token.json"],
            |_| AntigravityAuthStatus::Configured {
                method: "oauth-business".into(),
            },
        ),
        (
            "api key method",
            Some(settings("gemini-api-key")),
            file,
            &[],
            |_| AntigravityAuthStatus::Configured {
                method: "gemini-api-key".into(),
            },
        ),
        (
            "personal, token in keychain",
            Some(settings("oauth-personal")),
            TokenStore::KeychainOrFile,
            &[],
            |_| AntigravityAuthStatus::Configured {
                method: "oauth-personal".into(),
            },
        ),
    ];
    for (name, settings, store, tokens, expected) in cases {
        let root = tempfile::tempdir().unwrap();
        let home = AntigravityHome::resolve(Some(root.path().into()), Path::new("/unused"), store);
        let dir = home.acp_dir().to_path_buf();
        std::fs::create_dir_all(&dir).unwrap();
        if let Some(settings) = settings {
            std::fs::write(dir.join("settings.json"), settings).unwrap();
        }
        for token in tokens {
            std::fs::write(dir.join(token), "{}").unwrap();
        }
        assert_eq!(home.status(), expected(&dir), "{name}");
    }

    let root = tempfile::tempdir().unwrap();
    let home = AntigravityHome::resolve(Some(root.path().into()), Path::new("/unused"), file);
    std::fs::create_dir_all(home.acp_dir()).unwrap();
    std::fs::write(home.settings_path(), "{not json").unwrap();
    assert!(matches!(
        home.status(),
        AntigravityAuthStatus::Unreadable { .. }
    ));
}
