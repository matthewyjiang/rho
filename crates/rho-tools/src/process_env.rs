use rho_sdk::ProcessEnvironment;
use tokio::process::Command;

/// Applies a process environment policy to a Tokio command builder.
pub(crate) fn apply_process_environment(command: &mut Command, environment: &ProcessEnvironment) {
    match environment {
        ProcessEnvironment::Empty => {
            command.env_clear();
        }
        ProcessEnvironment::InheritAll => {}
        ProcessEnvironment::InheritExcept { variable_names } => {
            for name in variable_names {
                command.env_remove(name);
            }
        }
        ProcessEnvironment::InheritListed { variable_names } => {
            command.env_clear();
            for name in variable_names {
                if let Ok(value) = std::env::var(name) {
                    command.env(name, value);
                }
            }
        }
        _ => {}
    }
}
