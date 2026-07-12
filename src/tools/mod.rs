#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod bash;
mod diff;
pub mod edit_file;
pub mod list_dir;
#[cfg(windows)]
pub mod powershell;
mod process;
pub mod read_file;
pub mod rtk;
pub mod skill;
pub mod web;
pub mod write_file;

use crate::{config::Config, tool::ToolRegistry};

pub fn registry(config: &Config) -> ToolRegistry {
    let mut r = ToolRegistry::new();
    r.register(list_dir::ListDir);
    r.register(read_file::ReadFile);
    r.register(write_file::WriteFile);
    r.register(edit_file::EditFile);
    let processes = process::ProcessManager::new(process::ProcessLimits::default());
    r.register(process::Process::new(processes.clone()));
    r.set_shutdown(processes);
    let rtk_enabled = config.rtk && rtk::is_available();
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    r.register(bash::Bash::new(rtk_enabled));
    #[cfg(windows)]
    r.register(powershell::PowerShell::new(rtk_enabled));
    r.register(skill::Skill);
    let (web_search, fetch_content) = web::access_tools(config);
    if web_search.is_available() {
        r.register(web_search);
    }
    r.register(fetch_content);
    r.register(web::GetSearchContent);
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_includes_available_web_access_tools() {
        let registry = registry(&Config::default());
        let names = registry
            .specs()
            .into_iter()
            .map(|spec| spec.name)
            .collect::<Vec<_>>();

        assert_eq!(
            names.contains(&"web_search".to_string()),
            web::access_tools(&Config::default()).0.is_available()
        );
        assert!(names.contains(&"fetch_content".to_string()));
        assert!(names.contains(&"get_search_content".to_string()));
        assert_eq!(
            names
                .iter()
                .filter(|name| name.as_str() == "process")
                .count(),
            1
        );
    }

    #[test]
    fn registry_omits_web_search_when_disabled() {
        let config = Config {
            web_search_provider: crate::config::SearchProvider::Disabled,
            ..Config::default()
        };
        let names = registry(&config)
            .specs()
            .into_iter()
            .map(|spec| spec.name)
            .collect::<Vec<_>>();

        assert!(!names.contains(&"web_search".to_string()));
        assert!(names.contains(&"fetch_content".to_string()));
        assert!(names.contains(&"get_search_content".to_string()));
        assert_eq!(
            names
                .iter()
                .filter(|name| name.as_str() == "process")
                .count(),
            1
        );
    }
}
