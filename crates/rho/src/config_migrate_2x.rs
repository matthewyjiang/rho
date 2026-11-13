//! Rewrites a 2.x `config.toml` into the 3.0 files.
//!
//! 3.0 rejects every key 2.x accepted under another name, so an upgrade runs
//! [`migrate_2x`] once: agent settings stay in `config.toml` at their 3.0
//! keys, `tui` client settings move to `clients/tui.toml`, the theme and
//! chords move to `tui.toml`, and plaintext web-search keys come out for the
//! credential store. Keys that did not move keep their comments and order.
//! [`super::upgrade_2x`] writes the files, backs up the original, and builds
//! the notice from the report.

use rho_providers::credentials::WebSearchCredential;
use toml_edit::{DocumentMut, Item, Value};

use super::document::{shown, ConfigDocument};
use super::{SearchBackend, WebSearchMode};

/// The 3.0 files one 2.x `config.toml` becomes.
pub(crate) struct Migrated {
    /// `config.toml`.
    pub(crate) agent: DocumentMut,
    /// `clients/tui.toml`.
    pub(crate) client_tui: DocumentMut,
    /// `tui.toml`.
    pub(crate) tui_local: DocumentMut,
    /// Web-search keys 2.x kept in plaintext, for the credential store.
    pub(crate) secrets: Vec<(WebSearchCredential, String)>,
    /// What changed, in the order it was applied.
    pub(crate) report: Vec<Change>,
}

/// One of the 3.0 files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MigratedFile {
    Agent,
    ClientTui,
    TuiLocal,
}

/// One change [`migrate_2x`] made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Change {
    /// The value moved unchanged.
    Moved {
        from: String,
        file: MigratedFile,
        to: String,
    },
    /// 3.0 spells the value 2.x accepted differently.
    Rewritten {
        key: String,
        from: String,
        to: String,
    },
    /// 2.x ignored the key, here because a grouped key overrode it.
    Dropped { key: String },
    /// A plaintext web-search key, now in [`Migrated::secrets`].
    Secret { key: String },
}

/// Flat and grouped 2.x keys whose value moves unchanged, as
/// `(2.x key, file, 3.0 key)`. Grouped keys come first: 2.x let a group
/// key win over its flat spelling, so a flat key whose destination is
/// already set is dropped.
const MOVES: &[(&str, MigratedFile, &str)] = &[
    (
        "display.show_reasoning_output",
        MigratedFile::ClientTui,
        "show_reasoning_output",
    ),
    ("display.zen_mode", MigratedFile::ClientTui, "zen_mode"),
    (
        "display.output_streaming",
        MigratedFile::ClientTui,
        "output_streaming",
    ),
    (
        "display.max_tool_output_lines",
        MigratedFile::ClientTui,
        "max_tool_output_lines",
    ),
    (
        "display.prompt_history_limit",
        MigratedFile::ClientTui,
        "prompt_history_limit",
    ),
    (
        "display.cache_miss_notices",
        MigratedFile::ClientTui,
        "cache_miss_notices",
    ),
    (
        "display.show_header_hints",
        MigratedFile::ClientTui,
        "show_header_hints",
    ),
    (
        "display.notifications",
        MigratedFile::ClientTui,
        "notifications",
    ),
    ("display.theme", MigratedFile::TuiLocal, "theme"),
    (
        "behavior.check_for_updates",
        MigratedFile::ClientTui,
        "check_for_updates",
    ),
    (
        "keybindings.editing_mode",
        MigratedFile::ClientTui,
        "editing_mode",
    ),
    ("keybindings", MigratedFile::TuiLocal, "keybindings"),
    ("provider", MigratedFile::Agent, "model.provider"),
    ("auth", MigratedFile::Agent, "model.auth"),
    ("reasoning", MigratedFile::Agent, "model.reasoning"),
    ("reasoning_effort", MigratedFile::Agent, "model.reasoning"),
    ("fast_mode", MigratedFile::Agent, "model.fast_mode"),
    (
        "favorite_models",
        MigratedFile::Agent,
        "model.favorite_models",
    ),
    (
        "max_output_bytes",
        MigratedFile::Agent,
        "output.max_output_bytes",
    ),
    (
        "auto_compact",
        MigratedFile::Agent,
        "compaction.auto_compact",
    ),
    (
        "compact_threshold_percent",
        MigratedFile::Agent,
        "compaction.compact_threshold_percent",
    ),
    (
        "compact_target_percent",
        MigratedFile::Agent,
        "compaction.compact_target_percent",
    ),
    (
        "enable_subagents",
        MigratedFile::Agent,
        "behavior.enable_subagents",
    ),
    (
        "permission_mode",
        MigratedFile::Agent,
        "behavior.permission_mode",
    ),
    ("rtk", MigratedFile::Agent, "behavior.rtk"),
    ("inline_shell", MigratedFile::Agent, "behavior.inline_shell"),
    (
        "web_search_provider",
        MigratedFile::Agent,
        "web_search.provider",
    ),
    (
        "show_reasoning_output",
        MigratedFile::ClientTui,
        "show_reasoning_output",
    ),
    ("zen_mode", MigratedFile::ClientTui, "zen_mode"),
    (
        "max_tool_output_lines",
        MigratedFile::ClientTui,
        "max_tool_output_lines",
    ),
    (
        "check_for_updates",
        MigratedFile::ClientTui,
        "check_for_updates",
    ),
];

/// Plaintext web-search keys, grouped spelling first.
const SECRETS: &[(&str, WebSearchCredential)] = &[
    ("web_search.openai_api_key", WebSearchCredential::OpenAi),
    ("web_search.exa_api_key", WebSearchCredential::Exa),
    ("web_search.brave_api_key", WebSearchCredential::Brave),
    ("web_search_openai_api_key", WebSearchCredential::OpenAi),
    ("web_search_exa_api_key", WebSearchCredential::Exa),
    ("web_search_brave_api_key", WebSearchCredential::Brave),
];

/// `[title]` and the flat `title_*` keys, which 2.x read as the
/// `session-title` internal agent unless that entry was set.
const TITLE_KEYS: &[(&str, &str)] = &[
    ("title.provider", "provider"),
    ("title.model", "model"),
    ("title.auth", "auth"),
    ("title_provider", "provider"),
    ("title_model", "model"),
    ("title_auth", "auth"),
];

const SESSION_TITLE_ENTRY: &str = "internal_agents.session-title";

/// Rewrites one 2.x `config.toml`. Never fails: a key 3.0 does not read
/// is dropped or rewritten to the nearest 3.0 value, and reported.
pub(crate) fn migrate_2x(doc: DocumentMut) -> Migrated {
    let mut agent = ConfigDocument::from(doc);
    let mut client_tui = ConfigDocument::default();
    let mut tui_local = ConfigDocument::default();
    let mut report = Vec::new();
    let mut secrets = Vec::new();

    // `model = "id"` was a flat model id; 3.0 holds it in `[model]`.
    if let Some(Item::Value(Value::String(model))) = agent.get("model").cloned() {
        agent.remove("model");
        agent.set_value("model.model", Value::String(model));
        report.push(Change::Moved {
            from: "model".into(),
            file: MigratedFile::Agent,
            to: "model.model".into(),
        });
    }

    for (from, file, to) in MOVES {
        let Some(item) = agent.get(from).cloned() else {
            continue;
        };
        agent.remove(from);
        let target = match file {
            MigratedFile::Agent => &mut agent,
            MigratedFile::ClientTui => &mut client_tui,
            MigratedFile::TuiLocal => &mut tui_local,
        };
        if target.get(to).is_some() {
            report.push(Change::Dropped {
                key: (*from).into(),
            });
            continue;
        }
        target.set_item(to, Some(item));
        report.push(Change::Moved {
            from: (*from).into(),
            file: *file,
            to: (*to).into(),
        });
    }

    for (key, credential) in SECRETS {
        let Some(item) = agent.get(key).cloned() else {
            continue;
        };
        agent.remove(key);
        let secret = item.as_str().map(str::trim).unwrap_or_default();
        if secret.is_empty() || secrets.iter().any(|(saved, _)| saved == credential) {
            report.push(Change::Dropped { key: (*key).into() });
            continue;
        }
        secrets.push((*credential, secret.to_owned()));
        report.push(Change::Secret { key: (*key).into() });
    }

    let title_entry_set = agent.get(SESSION_TITLE_ENTRY).is_some();
    for (key, leaf) in TITLE_KEYS {
        let Some(item) = agent.get(key).cloned() else {
            continue;
        };
        agent.remove(key);
        let to = format!("{SESSION_TITLE_ENTRY}.{leaf}");
        if title_entry_set || agent.get(&to).is_some() {
            report.push(Change::Dropped { key: (*key).into() });
            continue;
        }
        agent.set_item(&to, Some(item));
        report.push(Change::Moved {
            from: (*key).into(),
            file: MigratedFile::Agent,
            to,
        });
    }

    migrate_web_search(&mut agent, &mut report);

    // 2.x saved this retired opt-in on every write and ignored it on load.
    const RETIRED_REWIND: &str = "behavior.experimental_workspace_rewind";
    if agent.get(RETIRED_REWIND).is_some() {
        agent.remove(RETIRED_REWIND);
        report.push(Change::Dropped {
            key: RETIRED_REWIND.into(),
        });
    }

    // The hashline edit format is gone; 2.x loaded it as str_replace.
    let edit_tools = agent
        .leaf_paths()
        .into_iter()
        .filter(|path| {
            path.last().is_some_and(|leaf| leaf == "edit_tool")
                && ((path.len() == 2 && path[0] == "behavior")
                    || (path.len() == 4 && path[0] == "providers" && path[1] == "custom"))
        })
        .map(|path| path.join("."))
        .collect::<Vec<_>>();
    for key in edit_tools {
        let hashline = agent
            .get(&key)
            .and_then(Item::as_str)
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("hashline"));
        if hashline {
            agent.set_value(&key, Value::from("str_replace"));
            report.push(Change::Rewritten {
                key,
                from: "\"hashline\"".into(),
                to: "\"str_replace\"".into(),
            });
        }
    }

    Migrated {
        agent: agent.into(),
        client_tui: client_tui.into(),
        tui_local: tui_local.into(),
        secrets,
        report,
    }
}

/// Maps 2.x `web_search.hosted` + `web_search.provider` onto `mode` and
/// `backend`, the way the 2.x loader read them.
///
/// `hosted = true` with `provider = "disabled"` meant native-only search,
/// which 3.0 cannot express without enabling a backend. It becomes `off`
/// rather than search through a backend the user had disabled.
fn migrate_web_search(agent: &mut ConfigDocument, report: &mut Vec<Change>) {
    const HOSTED: &str = "web_search.hosted";
    const PROVIDER: &str = "web_search.provider";
    let hosted_item = agent.get(HOSTED).cloned();
    let provider_item = agent.get(PROVIDER).cloned();
    if hosted_item.is_none() && provider_item.is_none() {
        return;
    }
    if agent.get("web_search.mode").is_some() {
        for key in [HOSTED, PROVIDER] {
            if agent.get(key).is_some() {
                agent.remove(key);
                report.push(Change::Dropped { key: key.into() });
            }
        }
        return;
    }
    let hosted = hosted_item.as_ref().and_then(Item::as_bool).unwrap_or(true);
    let provider = provider_item
        .as_ref()
        .and_then(Item::as_str)
        .map(|value| value.trim().to_ascii_lowercase());
    let (mode, backend) = match provider.as_deref() {
        Some("disabled") => (WebSearchMode::Off, None),
        Some("exa") => (legacy_mode(hosted), Some(SearchBackend::Exa)),
        Some("brave") => (legacy_mode(hosted), Some(SearchBackend::Brave)),
        // `auto`, `openai`, unset, and names 2.x did not know all searched
        // through OpenAI.
        _ => (legacy_mode(hosted), Some(SearchBackend::OpenAi)),
    };
    let from = [&hosted_item, &provider_item]
        .into_iter()
        .zip([HOSTED, PROVIDER])
        .filter_map(|(item, key)| {
            item.as_ref()
                .and_then(Item::as_value)
                .map(|value| format!("{key} = {}", shown(value)))
        })
        .collect::<Vec<_>>()
        .join(", ");
    agent.remove(HOSTED);
    agent.remove(PROVIDER);
    agent.set_value("web_search.mode", Value::from(mode.as_str()));
    let mut to = format!("web_search.mode = \"{}\"", mode.as_str());
    if let Some(backend) = backend.filter(|_| agent.get("web_search.backend").is_none()) {
        agent.set_value("web_search.backend", Value::from(backend.as_str()));
        to.push_str(&format!(", web_search.backend = \"{}\"", backend.as_str()));
    }
    report.push(Change::Rewritten {
        key: "web_search".into(),
        from,
        to,
    });
}

fn legacy_mode(hosted: bool) -> WebSearchMode {
    if hosted {
        WebSearchMode::Auto
    } else {
        WebSearchMode::Backend
    }
}

#[cfg(test)]
#[path = "config_migrate_2x_tests.rs"]
mod tests;
