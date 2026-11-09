//! A call in two steps: [`SessionControl::check`] parses its arguments and
//! applies every policy that does not depend on what else runs (the access
//! the action needs, the workspaces the grant reaches, the nesting depth),
//! and [`SessionControl::run`] admits it under the per-holder session cap
//! and acts. A front end that asks a person first, as the native `sessions`
//! tool does, checks before asking, so nobody approves a call the core would
//! refuse, and names the [`Target`] the check found.

use std::path::PathBuf;

use rho_protocol::SessionId;
use serde_json::{Map, Value};

use super::{
    canonical, dispatch::parse, prompt_content, Action, Caller, CancelParams, ControlError,
    ListParams, NewParams, PromptParams, ReadParams, Refusal, SessionControl, SteerParams,
    WaitParams,
};

/// A call that passed [`SessionControl::check`].
pub(crate) struct Checked {
    pub(super) call: CheckedCall,
    target: Option<Target>,
}

/// A checked call's parsed arguments.
pub(super) enum CheckedCall {
    List(ListParams),
    Read(ReadParams),
    /// `workspace` is the params' workspace, canonical.
    New {
        params: NewParams,
        workspace: PathBuf,
    },
    Prompt(PromptParams),
    Steer(SteerParams),
    Cancel(CancelParams),
    Wait(WaitParams),
}

/// The session a call acts on, for an approval to name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Target {
    pub(crate) action: Action,
    /// The session it prompts, steers, or cancels; `None` for one it starts.
    pub(crate) session: Option<SessionId>,
    /// The session's workspace, canonical.
    pub(crate) workspace: PathBuf,
    /// What it sends: the first prompt, or the prompt's or steer's text.
    pub(crate) text: Option<String>,
}

impl Checked {
    /// What the call acts on; `None` for an action that only watches.
    pub(crate) fn target(&self) -> Option<&Target> {
        self.target.as_ref()
    }
}

impl SessionControl {
    /// Parses `arguments` for `action` and checks them for `caller`:
    /// whether its grant allows the action and reaches the session's
    /// workspace, the session is one the daemon knows, a prompt is not
    /// empty, and a start or prompt stays within the nesting depth.
    pub(crate) async fn check(
        &self,
        caller: &Caller,
        action: Action,
        arguments: Map<String, Value>,
    ) -> Result<Checked, ControlError> {
        caller.grant.permit(action)?;
        let target = |session: Option<SessionId>, workspace, text| {
            Some(Target {
                action,
                session,
                workspace,
                text,
            })
        };
        let (call, target) = match action {
            Action::List => {
                let params: ListParams = parse(action, arguments)?;
                params.limit()?;
                (CheckedCall::List(params), None)
            }
            Action::Read => {
                let params: ReadParams = parse(action, arguments)?;
                params.window()?;
                self.reach_session(caller, &params.session).await?;
                (CheckedCall::Read(params), None)
            }
            Action::Wait => {
                let params: WaitParams = parse(action, arguments)?;
                params.timeout()?;
                params.cursor()?;
                self.reach_session(caller, &params.session).await?;
                (CheckedCall::Wait(params), None)
            }
            Action::New => {
                let params: NewParams = parse(action, arguments)?;
                let workspace = canonical(&params.workspace)?;
                caller.grant.reach(&workspace)?;
                prompt_content(&params.prompt)?;
                self.check_depth(caller)?;
                let target = target(None, workspace.clone(), Some(params.prompt.clone()));
                (CheckedCall::New { params, workspace }, target)
            }
            Action::Prompt => {
                let params: PromptParams = parse(action, arguments)?;
                let workspace = self.reach_session(caller, &params.session).await?;
                prompt_content(&params.text)?;
                self.check_depth(caller)?;
                let target = target(
                    Some(params.session.clone()),
                    workspace,
                    Some(params.text.clone()),
                );
                (CheckedCall::Prompt(params), target)
            }
            Action::Steer => {
                let params: SteerParams = parse(action, arguments)?;
                let workspace = self.reach_session(caller, &params.session).await?;
                let target = target(
                    Some(params.session.clone()),
                    workspace,
                    Some(params.text.clone()),
                );
                (CheckedCall::Steer(params), target)
            }
            Action::Cancel => {
                let params: CancelParams = parse(action, arguments)?;
                let workspace = self.reach_session(caller, &params.session).await?;
                let target = target(Some(params.session.clone()), workspace, None);
                (CheckedCall::Cancel(params), target)
            }
        };
        Ok(Checked { call, target })
    }

    /// Refuses a start or prompt deeper than [`super::Limits::max_depth`].
    fn check_depth(&self, caller: &Caller) -> Result<(), Refusal> {
        let asked = caller.depth.saturating_add(1);
        if asked > self.limits.max_depth {
            return Err(Refusal::Depth {
                limit: self.limits.max_depth,
                asked,
            });
        }
        Ok(())
    }
}
