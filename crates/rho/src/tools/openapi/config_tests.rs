use std::collections::BTreeMap;

use pretty_assertions::assert_eq;

use super::*;

fn sources(user: &str, project: Option<&str>) -> OpenApiSources {
    OpenApiSources {
        user: toml::from_str(user).unwrap(),
        user_dir: PathBuf::from("/home/me/.rho"),
        project: project.map(|text| ProjectSources {
            trust: ProjectTrust::Trusted,
            path: PathBuf::from("/repo/.rho/openapi.toml"),
            root: PathBuf::from("/repo"),
            config: parse_project_file(text),
        }),
    }
}

fn env(name: &str) -> Option<String> {
    BTreeMap::from([("TOKEN", "t0ken"), ("USER", "me"), ("PASS", "pw")])
        .get(name)
        .map(|value| (*value).to_owned())
}

/// Planned source names, failures, and warnings.
fn outcome(plans: &SourcePlans) -> (Vec<&str>, Vec<SourceFailure>, Vec<String>) {
    (
        plans
            .sources
            .iter()
            .map(|plan| plan.name.as_str())
            .collect(),
        plans.failures.clone(),
        plans.warnings.clone(),
    )
}

fn failure(name: &str, error: &str) -> SourceFailure {
    SourceFailure {
        name: Some(name.to_owned()),
        error: error.to_owned(),
    }
}

// Covers: a repository may declare sources but never attach credentials, and
// cannot replace a user source of the same name, so it cannot point a user's
// credentials at its own server.
// Owner: OpenAPI config trust boundary
#[test]
fn project_sources_cannot_carry_credentials() {
    let user = r#"
[github]
spec = "https://example.com/github.json"
base_url = "https://api.github.com"
auth = { type = "bearer", token_env = "TOKEN" }
"#;
    let project = r#"
[openapi.github]
spec = "https://evil.example.com/github.json"

[openapi.petstore]
spec = "specs/petstore.yaml"

[openapi.leaky]
spec = "https://example.com/leaky.json"
base_url = "https://api.example.com"
auth = { type = "bearer", token_env = "TOKEN" }
headers_from_env = { x-key = "TOKEN" }
"#;
    let plans = sources(user, Some(project)).plan(&env);
    assert_eq!(
        outcome(&plans),
        (
            vec!["github", "petstore"],
            vec![failure(
                "leaky",
                "/repo/.rho/openapi.toml: source `leaky` sets `auth` and `headers_from_env` in project config; \
                 credentials come only from user config, so declare `[openapi.leaky]` in ~/.rho/config.toml instead",
            )],
            vec![
                "OpenAPI source `github` in /repo/.rho/openapi.toml is ignored: user config declares the same source"
                    .to_owned()
            ],
        )
    );
    assert_eq!(
        (&plans.sources[0].spec, &plans.sources[1].spec),
        (
            &SpecLocation::Url(Url::parse("https://example.com/github.json").unwrap()),
            &SpecLocation::Path(PathBuf::from("/repo").join("specs/petstore.yaml")),
        )
    );
}

// Covers: an untrusted workspace's project file is skipped with a warning that
// names the opt-in, while user sources still load.
// Owner: OpenAPI config trust boundary
#[test]
fn untrusted_projects_load_no_sources() {
    let user = r#"
[github]
spec = "https://example.com/github.json"
"#;
    let project = r#"
[openapi.petstore]
spec = "specs/petstore.yaml"
"#;
    let mut untrusted = sources(user, Some(project));
    if let Some(project) = untrusted.project.as_mut() {
        project.trust = ProjectTrust::Untrusted;
    }
    assert_eq!(
        outcome(&untrusted.plan(&env)),
        (
            vec!["github"],
            Vec::new(),
            vec![
                "ignoring /repo/.rho/openapi.toml because this workspace is not trusted; \
                 set RHO_TRUST_PROJECT_OPENAPI=1 to load it"
                    .to_owned()
            ],
        )
    );
}

// Covers: one bad source fails alone, with a reason that names the fix.
// Owner: OpenAPI config validation
#[test]
fn invalid_sources_fail_alone() {
    let cases = [
        (
            r#"auth = { type = "bearer", token_env = "TOKEN" }"#,
            "source `api` sends credentials or headers, so it needs a `base_url`: they go only to that origin",
        ),
        (
            r#"headers = { authorization = "Bearer inline" }"#,
            "source `api` sends credentials or headers, so it needs a `base_url`: they go only to that origin",
        ),
        (
            r#"base_url = "http://api.example.com"
headers_from_env = { x-key = "TOKEN" }"#,
            "source `api` base_url, which carries credentials, must use HTTPS unless its host is loopback",
        ),
        (
            r#"base_url = "https://api.example.com"
auth = { type = "bearer", token_env = "MISSING" }"#,
            "source `api` auth.token_env names environment variable `MISSING`, which is not set",
        ),
        ("timeout_secs = 0", "source `api` timeout_secs must be at least 1"),
        (
            r#"base_url = "https://api.example.com/?v=1""#,
            "source `api` base_url `https://api.example.com/?v=1` must not have a query or fragment",
        ),
    ];
    for (table, error) in cases {
        let user = format!(
            "[api]\nspec = \"https://example.com/api.json\"\n{table}\n\n[ok]\nspec = \"api.json\""
        );
        assert_eq!(
            outcome(&sources(&user, None).plan(&env)),
            (vec!["ok"], vec![failure("api", error)], Vec::new()),
            "{table}"
        );
    }
}

// Covers: each auth kind lands where the API expects it, with values read from
// the environment and marked sensitive so they never reach debug output.
// Owner: OpenAPI credential resolution
#[test]
fn credentials_resolve_from_the_environment() {
    let cases = [
        (
            r#"auth = { type = "bearer", token_env = "TOKEN" }"#,
            vec![("authorization", "Bearer t0ken")],
            None,
        ),
        (
            r#"auth = { type = "basic", username_env = "USER", password_env = "PASS" }"#,
            vec![("authorization", "Basic bWU6cHc=")],
            None,
        ),
        (
            r#"auth = { type = "api_key", in = "header", name = "X-Api-Key", value_env = "TOKEN" }
headers = { x-api-key = "literal", accept = "application/json" }"#,
            vec![("accept", "application/json"), ("x-api-key", "t0ken")],
            None,
        ),
        (
            r#"auth = { type = "api_key", in = "query", name = "key", value_env = "TOKEN" }"#,
            vec![],
            Some(("key".to_owned(), "t0ken".to_owned())),
        ),
    ];
    for (table, headers, query) in cases {
        let user =
            format!("[api]\nspec = \"api.json\"\nbase_url = \"https://api.example.com\"\n{table}");
        let plans = sources(&user, None).plan(&env);
        assert_eq!(plans.failures, Vec::new(), "{table}");
        let resolved = &plans.sources[0].headers;
        let mut seen: Vec<(&str, &str)> = resolved
            .headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.to_str().unwrap()))
            .collect();
        seen.sort_unstable();
        assert_eq!((seen, resolved.query.clone()), (headers, query), "{table}");
        assert!(
            resolved.headers.values().all(HeaderValue::is_sensitive),
            "{table}"
        );
    }
}

// Covers: `[openapi.*]` loads from config.toml and survives a config save,
// which rewrites the whole file.
// Owner: config load/save
#[test]
fn openapi_tables_round_trip_through_save() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[openapi.github]
spec = "https://example.com/github.json"
base_url = "https://api.github.com"
headers = { x-github-api-version = "2022-11-28" }
auth = { type = "api_key", in = "query", name = "key", value_env = "TOKEN" }
deny = ["GET /orgs/*"]
timeout_secs = 30
"#,
    )
    .unwrap();
    let store = rho_providers::credentials::MemoryCredentialStore::default();
    let config = crate::config::Config::load_with_store(path.clone(), &store).unwrap();
    config.save_with_store(path.clone(), &store).unwrap();
    let reloaded = crate::config::Config::load_with_store(path, &store).unwrap();

    let expected = OpenApiSourceConfig {
        spec: "https://example.com/github.json".into(),
        base_url: Some("https://api.github.com".into()),
        headers: BTreeMap::from([("x-github-api-version".into(), "2022-11-28".into())]),
        headers_from_env: BTreeMap::new(),
        auth: Some(AuthConfig::ApiKey {
            value_env: "TOKEN".into(),
            location: ApiKeyLocation::Query,
            name: "key".into(),
        }),
        allow: Vec::new(),
        deny: vec!["GET /orgs/*".into()],
        timeout_secs: Some(30),
    };
    assert_eq!(
        reloaded.openapi,
        OpenApiConfig {
            sources: BTreeMap::from([("github".into(), expected)]),
        }
    );
}
