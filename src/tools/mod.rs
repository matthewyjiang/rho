#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod bash;
pub mod edit_file;
pub mod list_dir;
#[cfg(windows)]
pub mod powershell;
pub mod read_file;
pub mod skill;
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
    r
}
