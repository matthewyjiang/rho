pub mod edit_file;
pub mod list_dir;
pub mod read_file;
pub mod run_command;
pub mod write_file;

use crate::tool::ToolRegistry;

pub fn registry() -> ToolRegistry {
    let mut r = ToolRegistry::new();
    r.register(list_dir::ListDir);
    r.register(read_file::ReadFile);
    r.register(write_file::WriteFile);
    r.register(edit_file::EditFile);
    r.register(run_command::RunCommand);
    r
}
