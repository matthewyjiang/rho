use super::*;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

fn model(provider: &str, model: &str) -> PromptModel {
    PromptModel::Rho {
        provider: provider.to_owned(),
        model: model.to_owned(),
    }
}

fn catalog(files: &[(&str, &str)]) -> TempDir {
    let home = tempfile::tempdir().unwrap();
    let directory = home.path().join(".rho/model-prompts");
    fs::create_dir_all(&directory).unwrap();
    for (name, content) in files {
        fs::write(directory.join(name), content).unwrap();
    }
    home
}

// Covers schema errors that must not silently select a default or partial prompt.
// Owner: model prompt parser; no existing catalog parser covers these files.
#[test]
fn rejects_invalid_frontmatter_and_empty_bodies() {
    for source in [
        "body without frontmatter",
        "\n---\nprovider: p\nmodel: m\n---\nbody",
        "---\nprovider: p\nmodel: m\nbody",
        "---\n---\nbody",
        "---\nprovider: p\n---\nbody",
        "---\nmodel: m\n---\nbody",
        "---\nprovider: ''\nmodel: m\n---\nbody",
        "---\nprovider: p\nmodel: ' '\n---\nbody",
        "---\nprovider: ' p'\nmodel: m\n---\nbody",
        "---\nprovider: p\nmodel: 123\n---\nbody",
        "---\nprovider: null\nmodel: m\n---\nbody",
        "---\nprovider: p\nmodel: m\nmode: invalid\n---\nbody",
        "---\nprovider: p\nmodel: m\nmode: null\n---\nbody",
        "---\nprovider: p\nmodel: m\nunknown: true\n---\nbody",
        "---\nprovider: p\nprovider: q\nmodel: m\n---\nbody",
        "---\nprovider: [\nmodel: m\n---\nbody",
        "---\nprovider: p\nmodel: m\n---",
        "---\nprovider: p\nmodel: m\n---\n \t\n",
    ] {
        assert!(parse(Path::new("test.md"), source).is_err(), "{source:?}");
    }
}

#[test]
fn preserves_body_and_hashes_complete_source() {
    for (mode_field, mode) in [
        ("", ModelPromptMode::Append),
        ("mode: append\n", ModelPromptMode::Append),
        ("mode: replace\n", ModelPromptMode::Replace),
    ] {
        for newline in ["\n", "\r\n"] {
            let body = "\n  Keep formatting.\n---\n".replace('\n', newline);
            let source = format!("---\nprovider: p\nmodel: org/model:tag\n{mode_field}---\n")
                .replace('\n', newline)
                + &body;
            let path = Path::new("arbitrary.md");
            let (_, actual) = parse(path, &source).unwrap();
            assert_eq!(
                actual,
                ModelPrompt {
                    path: path.to_owned(),
                    mode,
                    body,
                    sha256: format!("{:x}", Sha256::digest(source.as_bytes())),
                }
            );
        }
    }
}

// Covers exact identity selection independent of filenames and nonrecursive discovery.
#[test]
fn discovery_matches_exact_identity_only() {
    let source = "---\nprovider: Provider\nmodel: org/model\n---\nbody\n";
    let home = catalog(&[
        ("arbitrary name.md", source),
        (
            "another-provider.md",
            "---\nprovider: Other\nmodel: org/model\n---\nother provider",
        ),
        (
            "another-model.md",
            "---\nprovider: Provider\nmodel: another/model\n---\nother model",
        ),
        ("ignored.txt", "malformed"),
    ]);
    let nested = home.path().join(".rho/model-prompts/nested.md");
    fs::create_dir(&nested).unwrap();
    fs::write(nested.join("invalid.md"), "malformed").unwrap();
    let path = home.path().join(".rho/model-prompts/arbitrary name.md");
    let expected = parse(&path, source).unwrap().1;
    for (provider, id, selected) in [
        ("Provider", "org/model", Some(expected)),
        ("provider", "org/model", None),
        ("Provider", "model", None),
        ("Provider", "org/model-extra", None),
    ] {
        assert_eq!(
            load(Some(home.path()), &model(provider, id)).unwrap(),
            selected
        );
    }
}

// Covers late malformed files and duplicates even when neither identity is selected.
#[test]
fn validates_entire_catalog_before_returning() {
    let valid = "---\nprovider: p\nmodel: m\n---\nbody";
    let unrelated = "---\nprovider: other\nmodel: m\n---\nbody";
    for files in [
        vec![("a.md", valid), ("z.md", "malformed")],
        vec![("a.md", valid), ("z.md", valid)],
        vec![("a.md", unrelated), ("z.md", unrelated)],
    ] {
        let home = catalog(&files);
        assert!(load(Some(home.path()), &model("p", "m")).is_err());
    }
}

#[test]
fn missing_catalog_is_optional_but_invalid_directory_is_an_io_error() {
    let home = tempfile::tempdir().unwrap();
    let target = model("p", "m");
    for path in [None, Some(home.path())] {
        assert_eq!(load(path, &target).unwrap(), None);
    }
    fs::create_dir(home.path().join(".rho")).unwrap();
    fs::write(home.path().join(".rho/model-prompts"), "not a directory").unwrap();
    let error = load(Some(home.path()), &target).unwrap_err();
    assert!(error.downcast_ref::<std::io::Error>().is_some());
    let external = PromptModel::ExternalCli {
        runtime: crate::agent::AgentRuntime::ClaudeCli,
        requested: None,
        resolved: None,
    };
    assert_eq!(load(Some(home.path()), &external).unwrap(), None);
}

#[test]
fn unreadable_markdown_is_not_silently_skipped() {
    let home = catalog(&[]);
    fs::write(home.path().join(".rho/model-prompts/invalid.md"), [0xff]).unwrap();
    let error = load(Some(home.path()), &model("p", "m")).unwrap_err();
    assert_eq!(
        error.downcast_ref::<std::io::Error>().unwrap().kind(),
        ErrorKind::InvalidData
    );
}
