#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod bash;
pub mod edit_file;
pub mod list_dir;
#[cfg(windows)]
pub mod powershell;
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
    let rtk_enabled = config.rtk && rtk::is_available();
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    r.register(bash::Bash::new(rtk_enabled));
    #[cfg(windows)]
    r.register(powershell::PowerShell::new(rtk_enabled));
    r.register(skill::Skill);
    let web_search = web::WebSearch::from_config(config);
    if web_search.is_available() {
        r.register(web_search);
    }
    r.register(web::FetchContent);
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
            web::WebSearch::from_config(&Config::default()).is_available()
        );
        assert!(names.contains(&"fetch_content".to_string()));
        assert!(names.contains(&"get_search_content".to_string()));
    }

    #[test]
    fn registry_omits_web_search_when_disabled() {
        let config = Config {
            web_search_provider: "disabled".into(),
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
    }
}
