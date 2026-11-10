use std::collections::BTreeMap;

use pretty_assertions::assert_eq;
use toml_edit::{Item, Value};

use super::{encode_table, ConfigDocument};

const COMMENTED: &str = r#"# my config
[model] # the conversation model
provider = "openai" # keep this
model = "gpt-5.5"

[behavior]
# who decides
permission_mode = "ask"
"#;

// Covers: each edit changes only what it names; comments, order, and
// quoting elsewhere survive, and removals clean up only uncommented tables.
// Owner: config document
#[test]
fn edits_keep_everything_they_do_not_name() {
    struct Case {
        name: &'static str,
        before: &'static str,
        edit: fn(&mut ConfigDocument),
        after: String,
    }
    let cases = [
        Case {
            name: "a replaced value keeps its trailing comment",
            before: COMMENTED,
            edit: |doc| doc.set_value("model.provider", Value::from("anthropic")),
            after: COMMENTED.replace("\"openai\"", "\"anthropic\""),
        },
        Case {
            name: "an equal value stays as written",
            before: "provider = 'openai'   # literal\n",
            edit: |doc| doc.set_value("provider", Value::from("openai")),
            after: "provider = 'openai'   # literal\n".into(),
        },
        Case {
            name: "removing the last key drops an uncommented table",
            before: COMMENTED,
            edit: |doc| doc.remove("behavior.permission_mode"),
            after: "# my config\n[model] # the conversation model\nprovider = \"openai\" # keep this\nmodel = \"gpt-5.5\"\n".into(),
        },
        Case {
            name: "removing the last key keeps a commented table",
            before: COMMENTED,
            edit: |doc| {
                doc.remove("model.provider");
                doc.remove("model.model");
            },
            after: "# my config\n[model] # the conversation model\n\n[behavior]\n# who decides\npermission_mode = \"ask\"\n".into(),
        },
        Case {
            name: "writing a table edits only the entries that differ",
            before: COMMENTED,
            edit: |doc| {
                let provider_only = BTreeMap::from([("provider", "openai")]);
                doc.set_item("model", Some(encode_table(&provider_only).unwrap()));
            },
            after: COMMENTED.replace("model = \"gpt-5.5\"\n", ""),
        },
    ];
    for case in cases {
        let mut doc = ConfigDocument::parse(case.before).unwrap();
        (case.edit)(&mut doc);
        assert_eq!(doc.text(), case.after, "{}", case.name);
    }
}

// Covers: dotted keys reach values in standard, dotted, and inline tables,
// and a write into an inline table keeps it inline.
// Owner: config document
#[test]
fn dotted_keys_walk_every_table_form() {
    let mut doc = ConfigDocument::parse(
        "[web_search]\nmode = \"auto\"\nexa.connection = \"mcp\"\n[xai]\nopenai = { connection = \"codex\" }\n",
    )
    .unwrap();
    doc.set_value("xai.openai.api_base_url", Value::from("https://x.example"));

    let read = |key: &str| doc.get(key).and_then(Item::as_str).map(str::to_owned);
    assert_eq!(
        (
            read("web_search.mode"),
            read("web_search.exa.connection"),
            read("xai.openai.connection"),
            read("xai.openai.api_base_url"),
            read("xai.missing"),
            doc.get("xai.openai").is_some_and(Item::is_inline_table),
        ),
        (
            Some("auto".into()),
            Some("mcp".into()),
            Some("codex".into()),
            Some("https://x.example".into()),
            None,
            true,
        )
    );
}

// Covers: an encoded structured table is written the way a person would
// write it, with `[header]` tables instead of `{ inline }` ones.
// Owner: config document
#[test]
fn encoded_tables_use_headers() {
    let providers = BTreeMap::from([(
        "custom",
        BTreeMap::from([(
            "local",
            BTreeMap::from([("base_url", "http://127.0.0.1/v1")]),
        )]),
    )]);
    let mut doc = ConfigDocument::default();
    doc.set_item("providers", Some(encode_table(&providers).unwrap()));
    assert_eq!(
        doc.text(),
        "[providers.custom.local]\nbase_url = \"http://127.0.0.1/v1\"\n"
    );
}
