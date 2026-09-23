use pretty_assertions::assert_eq;

use super::*;

// Covers: agents group by source with editable ones first, so "which can I
// change?" is answered by position; the catalog's alphabetical order would
// interleave sources.
// Owner: agents picker policy
#[test]
fn agents_group_by_source_with_editable_first() {
    let home = tempfile::tempdir().expect("tempdir");
    let rho_agents = home.path().join(".rho/agents");
    let shared_agents = home.path().join(".agents/agents");
    std::fs::create_dir_all(&rho_agents).expect("rho agents dir");
    std::fs::create_dir_all(&shared_agents).expect("shared agents dir");
    std::fs::write(
        rho_agents.join("zeta.md"),
        "---\ndescription: mine\n---\nbody\n",
    )
    .expect("write zeta");
    std::fs::write(
        shared_agents.join("alpha.md"),
        "---\ndescription: shared\n---\nbody\n",
    )
    .expect("write alpha");
    // A cwd outside home, so `~/.agents/agents` is not also a project dir.
    let cwd = tempfile::tempdir().expect("cwd");
    let catalog = AgentCatalog::discover_with_home(cwd.path(), Some(home.path())).expect("catalog");
    let config = crate::config::Config::default();

    let picker = agent_picker(catalog, AgentModelView::from(&config));

    let mut sections = picker
        .items
        .iter()
        .map(|item| item.section.as_deref().unwrap_or_default())
        .collect::<Vec<_>>();
    sections.dedup();
    assert_eq!(sections, vec!["YOURS", "SHARED", "BUILT IN", "INTERNAL"]);
    assert_eq!(picker.items[0].value, "zeta");
    assert_eq!(picker.items[1].value, "alpha");
}

// Covers: Enter on an internal agent configures its model rather than opening
// the read-only prompt view, so the detail pane must carry the whole prompt;
// other agents keep a short excerpt because Enter opens the full text.
// Owner: agents picker policy
#[test]
fn internal_agents_show_the_full_prompt_in_detail() {
    let home = tempfile::tempdir().expect("tempdir");
    let shared_agents = home.path().join(".agents/agents");
    std::fs::create_dir_all(&shared_agents).expect("shared agents dir");
    std::fs::write(
        shared_agents.join("alpha.md"),
        "---\ndescription: shared\n---\nbody\n",
    )
    .expect("write alpha");
    let cwd = tempfile::tempdir().expect("cwd");
    let catalog = AgentCatalog::discover_with_home(cwd.path(), Some(home.path())).expect("catalog");
    let config = crate::config::Config::default();

    let picker = agent_picker(catalog, AgentModelView::from(&config));

    let prompt_block = |id: &str| {
        let item = picker
            .items
            .iter()
            .find(|item| item.value == id)
            .unwrap_or_else(|| panic!("{id} listed"));
        let Some(crate::tui::picker::PickerDetail::Sheet(sheet)) = &item.detail else {
            panic!("{id} has a detail sheet");
        };
        sheet.blocks.last().cloned().expect("prompt block")
    };
    assert_eq!(
        prompt_block("goal-judge"),
        DetailBlock::Muted(crate::tui::GOAL_JUDGE_PROMPT.to_string())
    );
    assert!(matches!(
        prompt_block("alpha"),
        DetailBlock::Excerpt {
            rows: PROMPT_EXCERPT_ROWS,
            ..
        }
    ));
}
