use std::{
    ffi::OsString,
    sync::{Arc, PoisonError, RwLock},
};

use rho_sdk::ProcessEnvironment;
use tokio::process::Command;

/// The environment a session's processes start with, which the session's
/// host may replace between turns: a daemon session takes each prompting
/// client's environment (#1659). Clones share it; a tool call takes the
/// environment current when it is prepared, so the approved execution is
/// the one that runs.
#[derive(Clone, Debug)]
pub struct SharedProcessEnvironment(Arc<RwLock<ProcessEnvironment>>);

impl SharedProcessEnvironment {
    pub fn new(environment: ProcessEnvironment) -> Self {
        Self(Arc::new(RwLock::new(environment)))
    }

    /// The environment a process started now gets.
    pub fn current(&self) -> ProcessEnvironment {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Makes `environment` the one every later process starts with.
    pub fn replace(&self, environment: ProcessEnvironment) {
        *self.0.write().unwrap_or_else(PoisonError::into_inner) = environment;
    }
}

impl From<ProcessEnvironment> for SharedProcessEnvironment {
    fn from(environment: ProcessEnvironment) -> Self {
        Self::new(environment)
    }
}

/// Variable `name` as a process started with `environment` would see it, so
/// a tool can honor a setting of the session's environment without starting
/// a process. `None` for a variable the policy withholds, and for an unknown
/// [`ProcessEnvironment`] variant, which [`apply_process_environment`]
/// refuses.
pub(crate) fn process_variable(environment: &ProcessEnvironment, name: &str) -> Option<OsString> {
    let listed = |names: &[String]| names.iter().any(|listed| listed == name);
    match environment {
        ProcessEnvironment::InheritAll => std::env::var_os(name),
        ProcessEnvironment::InheritExcept { variable_names } => {
            (!listed(variable_names)).then(|| std::env::var_os(name))?
        }
        ProcessEnvironment::InheritListed { variable_names } => {
            listed(variable_names).then(|| std::env::var_os(name))?
        }
        ProcessEnvironment::Explicit { variables } => variables.get(name).map(OsString::from),
        ProcessEnvironment::Empty => None,
        _ => None,
    }
}

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
        ProcessEnvironment::Explicit { variables } => {
            command.env_clear();
            command.envs(variables.iter());
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
