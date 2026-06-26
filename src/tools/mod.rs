#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod bash;
pub mod edit_file;
pub mod list_dir;
#[cfg(windows)]
pub mod powershell;
pub mod read_file;
pub mod skill;
pub mod web;
pub mod write_file;

use crate::tool::ToolRegistry;

pub fn registry() -> ToolRegistry {
    let mut r = ToolRegistry::new();
    r.register(list_dir::ListDir);
    r.register(read_file::ReadFile);
    r.register(write_file::WriteFile);
    r.register(edit_file::EditFile);
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    r.register(bash::Bash);
    #[cfg(windows)]
    r.register(powershell::PowerShell);
    r.register(skill::Skill);
    r.register(web::WebSearch);
    r.register(web::FetchContent);
    r.register(web::GetSearchContent);
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_includes_web_access_tools() {
        let registry = registry();
        let names = registry
            .specs()
            .into_iter()
            .map(|spec| spec.name)
            .collect::<Vec<_>>();

        assert!(names.contains(&"web_search".to_string()));
        assert!(names.contains(&"fetch_content".to_string()));
        assert!(names.contains(&"get_search_content".to_string()));
    }
}
