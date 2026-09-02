//! `/thermos` starts the workspace thermo-nuclear-review workflow.

use std::collections::BTreeMap;

use ratatui::DefaultTerminal;

use super::{
    workflow_discover::{self, DiscoveredWorkflow},
    App, CommandInvocation, Entry, InteractiveRuntime,
};
use crate::workflow::{InputName, WorkflowValue};

const THERMOS_LABELS: &[&str] = &["thermo-nuclear-review", "thermos"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ThermosScope {
    All,
    Committed,
    Uncommitted,
}

impl ThermosScope {
    fn parse(token: &str) -> Option<Self> {
        match token {
            "all" => Some(Self::All),
            "committed" => Some(Self::Committed),
            "uncommitted" => Some(Self::Uncommitted),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Committed => "committed",
            Self::Uncommitted => "uncommitted",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ThermosRequest {
    scope: ThermosScope,
    focus_path: Option<String>,
}

impl ThermosRequest {
    fn into_inputs(self) -> BTreeMap<InputName, WorkflowValue> {
        let mut inputs = BTreeMap::new();
        if self.scope != ThermosScope::All {
            inputs.insert(
                input_name("scope"),
                WorkflowValue::String(self.scope.as_str().to_owned()),
            );
        }
        if let Some(focus_path) = self.focus_path {
            inputs.insert(input_name("focus_path"), WorkflowValue::String(focus_path));
        }
        inputs
    }
}

fn input_name(name: &str) -> InputName {
    InputName::new(name).expect("thermos input names are static portable ids")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ThermosArgsError {
    InvalidFlag,
}

pub(super) fn parse_thermos_args(args: &str) -> Result<ThermosRequest, ThermosArgsError> {
    let mut tokens = args.split_whitespace();
    let Some(first) = tokens.next() else {
        return Ok(ThermosRequest {
            scope: ThermosScope::All,
            focus_path: None,
        });
    };
    if first.starts_with('-') {
        return Err(ThermosArgsError::InvalidFlag);
    }
    if let Some(scope) = ThermosScope::parse(&first.to_ascii_lowercase()) {
        let rest = tokens.collect::<Vec<_>>();
        if rest.iter().any(|token| token.starts_with('-')) {
            return Err(ThermosArgsError::InvalidFlag);
        }
        let focus_path = if rest.is_empty() {
            None
        } else {
            Some(rest.join(" "))
        };
        return Ok(ThermosRequest { scope, focus_path });
    }
    let mut parts = vec![first];
    parts.extend(tokens);
    if parts.iter().any(|token| token.starts_with('-')) {
        return Err(ThermosArgsError::InvalidFlag);
    }
    Ok(ThermosRequest {
        scope: ThermosScope::All,
        focus_path: Some(parts.join(" ")),
    })
}

pub(super) fn find_thermos_workflow(sources: &[DiscoveredWorkflow]) -> Option<&DiscoveredWorkflow> {
    THERMOS_LABELS
        .iter()
        .find_map(|label| sources.iter().find(|source| source.label == *label))
}

impl App {
    pub(super) async fn execute_thermos_command(
        &mut self,
        invocation: CommandInvocation,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let request = match parse_thermos_args(&invocation.args) {
            Ok(request) => request,
            Err(ThermosArgsError::InvalidFlag) => {
                self.insert_entry(&Entry::Error(
                    "usage: /thermos [all|committed|uncommitted] [path]".into(),
                ));
                self.set_status("invalid thermos arguments");
                return Ok(());
            }
        };
        let sources = workflow_discover::discover_workflow_sources(&self.info.runtime.cwd);
        let Some(source) = find_thermos_workflow(&sources) else {
            self.insert_entry(&Entry::Error(
                "could not start thermos: no thermo-nuclear-review workflow in .rho/workflows"
                    .into(),
            ));
            self.set_status("thermos unavailable");
            return Ok(());
        };
        self.start_workflow_source_with_inputs(
            &source.relative_path,
            request.into_inputs(),
            terminal,
            agent,
        )
        .await
    }
}

#[cfg(test)]
#[path = "thermos_command_tests.rs"]
mod tests;
