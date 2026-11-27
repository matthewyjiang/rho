use pretty_assertions::assert_eq;
use rho_protocol::{TableChange, TableEntry};
use serde_json::{json, Value};

use super::{apply_table_changes, table_entries, EditableTable};
use crate::config::{ChangeError, ConfigDocument, ConfigError};

fn put(key: &str, value: Value) -> TableChange {
    TableChange::Put {
        key: key.into(),
        value,
    }
}

fn remove(key: &str) -> TableChange {
    TableChange::Remove { key: key.into() }
}

/// `text` with `changes` to `table` applied, as the file would read.
fn applied(text: &str, table: EditableTable, changes: &[TableChange]) -> String {
    let mut doc = ConfigDocument::parse(text).unwrap();
    apply_table_changes(&mut doc, table, changes).unwrap();
    doc.text()
}

// Covers: a put replaces the whole entry, dropping fields it leaves out, and
// a remove deletes one, while every other entry and the file's comments stay
// as written; `ollama` maps to its own table, which an empty profile keeps.
// Owner: config tables.
#[test]
fn a_put_replaces_the_whole_entry_and_leaves_the_rest_of_the_file() {
    let text = r#"# Mine.
[providers.custom.local]
# The box under the desk.
base_url = "http://127.0.0.1:8080/v1"
catalog = "openai"

[providers.custom.spare]
base_url = "http://127.0.0.1:9090/v1"
"#;
    assert_eq!(
        applied(
            text,
            EditableTable::Providers,
            &[
                put("local", json!({"base_url": "http://10.0.0.2:8080/v1"})),
                remove("spare"),
                put("ollama", json!({})),
            ],
        ),
        r#"# Mine.
[providers.custom.local]
# The box under the desk.
base_url = "http://10.0.0.2:8080/v1"

[providers.ollama]
"#
    );
}

// Covers: what the providers table lists is what a put or remove by the
// same key reaches: a custom table named after a built-in provider, which
// the built-in shadows, is not listed as the built-in's own profile.
// Owner: config tables.
#[test]
fn a_shadowed_custom_provider_is_not_listed_as_the_built_in() {
    let doc = ConfigDocument::parse(
        "[providers.custom.ollama]\nbase_url = \"http://127.0.0.1:11434/v1\"\n\n[providers.custom.local]\nbase_url = \"http://127.0.0.1:8080/v1\"\n",
    )
    .unwrap();
    assert_eq!(
        table_entries(&doc, EditableTable::Providers),
        [TableEntry::new(
            "local".into(),
            json!({"base_url": "http://127.0.0.1:8080/v1"})
        )]
    );
}

// Covers: favorites keep their order as the loader reads them: a new one
// goes last, one already listed, however it is spelled, keeps its place, and
// a removed one leaves the rest in order.
// Owner: config tables.
#[test]
fn favorites_keep_their_order() {
    let text = "[model]\nfavorite_models = [\"xai/grok-4\", \"openai/gpt-5.5\"]\n";
    let mut doc = ConfigDocument::parse(text).unwrap();
    apply_table_changes(
        &mut doc,
        EditableTable::FavoriteModels,
        &[
            put("anthropic/claude-sonnet-4-5", Value::Null),
            put(" xai/grok-4 ", Value::Null),
            remove("openai/gpt-5.5"),
        ],
    )
    .unwrap();
    assert_eq!(
        table_entries(&doc, EditableTable::FavoriteModels),
        ["xai/grok-4", "anthropic/claude-sonnet-4-5"]
            .map(|key| TableEntry::new(key.into(), Value::Null))
    );
}

// Covers: every change the loader refuses is named, by entry, and nothing is
// applied.
// Owner: config tables.
#[test]
fn a_refused_change_names_its_entry_and_applies_nothing() {
    let text = "[model.aliases]\nfast = \"openai/gpt-5.5\"\n";
    let cases = [
        (
            EditableTable::Providers,
            put("local", json!({"base_url": "http://x", "colour": "red"})),
            "providers.custom.local",
        ),
        (
            EditableTable::ModelAliases,
            put("fast", json!({"model": "gpt-5.5"})),
            "model.aliases.fast",
        ),
        (
            EditableTable::ModelAliases,
            remove("grok.v2"),
            "model.aliases.\"grok.v2\"",
        ),
        (
            EditableTable::FavoriteModels,
            put("gpt-5.5", Value::Null),
            "model.favorite_models[\"gpt-5.5\"]",
        ),
        (
            EditableTable::FavoriteModels,
            put("openai/gpt-5.5", json!(true)),
            "model.favorite_models[\"openai/gpt-5.5\"]",
        ),
    ];
    for (table, change, key) in cases {
        let mut doc = ConfigDocument::parse(text).unwrap();
        let refused = apply_table_changes(&mut doc, table, std::slice::from_ref(&change));
        let Err(ChangeError::Refused(errors)) = refused else {
            panic!("{change:?} was not refused: {refused:?}");
        };
        assert_eq!(
            errors
                .iter()
                .map(|error| error.key.as_deref())
                .collect::<Vec<_>>(),
            [Some(key)],
            "{change:?}: {errors:?}"
        );
        assert_eq!(doc.text(), text);
    }
}

// Covers: a file that does not load for keys the changes leave alone is
// reported as invalid; one broken in an entry the changes replace reports
// the changes' own faults as refused; and changes that repair a file land.
// Owner: config tables.
#[test]
fn only_a_file_broken_elsewhere_is_invalid_and_one_the_changes_repair_saves() {
    let broken = "[model.aliases]\nfast = \"openai/gpt-5.5\"\n\n[behavior]\nno_such_key = 1\n";
    let mut doc = ConfigDocument::parse(broken).unwrap();
    assert!(matches!(
        apply_table_changes(
            &mut doc,
            EditableTable::ModelAliases,
            &[put("slow", json!("openai/gpt-5.5"))],
        ),
        Err(ChangeError::FileInvalid(errors)) if errors != Vec::<ConfigError>::new()
    ));

    // Broken only in the entry the changes replace: the new entry's own
    // fault is the change's.
    let broken_entry = "[providers.custom.local]\ncatalog = \"openai\"\n";
    let mut doc = ConfigDocument::parse(broken_entry).unwrap();
    assert!(matches!(
        apply_table_changes(
            &mut doc,
            EditableTable::Providers,
            &[
                put("local", json!({"base_url": "http://127.0.0.1:8080/v1"})),
                put("spare", json!({})),
            ],
        ),
        Err(ChangeError::Refused(_))
    ));

    let dangling = "[model]\nmodel = \"@fast\"\n";
    assert_eq!(
        applied(
            dangling,
            EditableTable::ModelAliases,
            &[put("fast", json!("openai/gpt-5.5"))]
        ),
        "[model]\nmodel = \"@fast\"\n\n[model.aliases]\nfast = \"openai/gpt-5.5\"\n"
    );
}
