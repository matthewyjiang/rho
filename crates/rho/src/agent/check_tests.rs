use std::path::Path;

use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::{check_agents, AgentCheckReport, AgentCheckStatus, AgentDirs, CheckedAgent};
use crate::workspace::ProjectTrust;

fn write_agent(dir: &Path, name: &str, contents: &str) -> String {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, contents).unwrap();
    crate::paths::display(&path)
}

fn dirs(home: &Path) -> AgentDirs {
    let [agents_home, rho_home] = crate::paths::user_agent_dirs(home);
    AgentDirs {
        agents_home: Some(crate::paths::display(&agents_home)),
        rho_home: Some(crate::paths::display(&rho_home)),
        project: None,
    }
}

// Covers: the creator's post-write check must report the file the next
// session would load, or the exact file and field that would break discovery.
// Owner: agent check
#[test]
fn reports_loaded_agents_or_the_first_invalid_file() {
    let cwd = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let rho_agents = home.path().join(".rho/agents");
    let valid = write_agent(
        &rho_agents,
        "reviewer.md",
        "---\ndescription: reviews\ntools: [read_file]\n---\nReview.\n",
    );

    assert_eq!(
        check_agents(cwd.path(), Some(home.path()), ProjectTrust::Untrusted),
        AgentCheckReport {
            dirs: dirs(home.path()),
            status: AgentCheckStatus::Ok {
                agents: vec![CheckedAgent {
                    id: "reviewer".into(),
                    origin: "rho_home",
                    path: valid,
                }],
            },
        }
    );

    let invalid = write_agent(
        &rho_agents,
        "broken.md",
        "---\ndescription: broken\nruntime: antigravity\ntools: all\n---\nBody.\n",
    );
    let report = check_agents(cwd.path(), Some(home.path()), ProjectTrust::Untrusted);
    let AgentCheckStatus::Error { path, field, .. } = report.status else {
        panic!("expected an error, got {:?}", report.status);
    };
    assert_eq!((path, field), (invalid, Some("tools".to_string())));
}
