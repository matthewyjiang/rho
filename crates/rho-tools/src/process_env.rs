use rho_sdk::ProcessEnvironment;
use tokio::process::Command;

/// Applies a process environment policy to a Tokio command builder.
///
/// Returns an error for unknown [`ProcessEnvironment`] variants so callers fail
/// closed instead of silently inheriting ambient state.
pub fn apply_process_environment(
    command: &mut Command,
    environment: &ProcessEnvironment,
) -> Result<(), String> {
    match environment {
        ProcessEnvironment::Empty => {
            command.env_clear();
            Ok(())
        }
        ProcessEnvironment::InheritAll => Ok(()),
        ProcessEnvironment::InheritExcept { variable_names } => {
            for name in variable_names {
                command.env_remove(name);
            }
            Ok(())
        }
        ProcessEnvironment::InheritListed { variable_names } => {
            command.env_clear();
            for name in variable_names {
                if let Ok(value) = std::env::var(name) {
                    command.env(name, value);
                }
            }
            Ok(())
        }
        _ => Err("unsupported process environment policy".into()),
    }
}
