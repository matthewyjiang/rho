//! A call in two steps: [`SessionControl::check`] parses its arguments and
//! applies every policy that does not depend on what else runs (the access
//! the action needs, the workspaces the grant reaches, the nesting depth),
//! and [`SessionControl::run`] admits it under the per-holder session cap
//! and acts. A front end that asks a person first, as the native `sessions`
//! tool does, checks before asking, so nobody approves a call the core would
//! refuse, and names the [`Target`] the check found.

use std::path::PathBuf;

use rho_protocol::{Ask, InputRequestId, QuestionAnswer, SessionId};
use serde_json::{Map, Value};

use super::{
    canonical, dispatch::parse_arguments, prompt_content, Action, AnswerParams, Caller,
    CancelParams, ControlError, ListParams, NewParams, PromptParams, ReadParams, Refusal,
    SessionControl, SteerParams, WaitParams,
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
    /// `by`, the calling session, gives `answer` to question `request` of
    /// `session`.
    Answer {
        session: SessionId,
        request: InputRequestId,
        answer: QuestionAnswer,
        by: SessionId,
    },
}

/// The session a call acts on, for an approval to name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Target {
    pub(crate) action: Action,
    /// The session it prompts, steers, cancels, or answers; `None` for one
    /// it starts.
    pub(crate) session: Option<SessionId>,
    /// The session's workspace, canonical.
    pub(crate) workspace: PathBuf,
    /// What it sends: the first prompt, the prompt's or steer's text, or
    /// the answer with the request it answers.
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
    /// empty, a start or prompt stays within the nesting depth, and an
    /// answer gives answers or declines, from a calling session. Whether
    /// that session may answer is the daemon's to decide, when it answers.
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
                let params: ListParams = parse_arguments(action.name(), arguments)?;
                params.limit()?;
                (CheckedCall::List(params), None)
            }
            Action::Read => {
                let params: ReadParams = parse_arguments(action.name(), arguments)?;
                params.window()?;
                self.reach_session(caller, &params.session).await?;
                (CheckedCall::Read(params), None)
            }
            Action::Wait => {
                let params: WaitParams = parse_arguments(action.name(), arguments)?;
                params.timeout()?;
                params.cursor()?;
                self.reach_session(caller, &params.session).await?;
                (CheckedCall::Wait(params), None)
            }
            Action::New => {
                let params: NewParams = parse_arguments(action.name(), arguments)?;
                let workspace = canonical(&params.workspace)?;
                caller.grant.reach(&workspace)?;
                prompt_content(&params.prompt)?;
                self.check_depth(caller)?;
                let target = target(None, workspace.clone(), Some(params.prompt.clone()));
                (CheckedCall::New { params, workspace }, target)
            }
            Action::Prompt => {
                let params: PromptParams = parse_arguments(action.name(), arguments)?;
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
                let params: SteerParams = parse_arguments(action.name(), arguments)?;
                let workspace = self.reach_session(caller, &params.session).await?;
                let target = target(
                    Some(params.session.clone()),
                    workspace,
                    Some(params.text.clone()),
                );
                (CheckedCall::Steer(params), target)
            }
            Action::Cancel => {
                let params: CancelParams = parse_arguments(action.name(), arguments)?;
                let workspace = self.reach_session(caller, &params.session).await?;
                let target = target(Some(params.session.clone()), workspace, None);
                (CheckedCall::Cancel(params), target)
            }
            Action::Answer => {
                let params: AnswerParams = parse_arguments(action.name(), arguments)?;
                let answer = params.answer()?;
                let by = caller.session.clone().ok_or_else(|| {
                    ControlError::Invalid(
                        "`answer` needs a calling session: only the session that started another may answer its questions".into(),
                    )
                })?;
                let workspace = self.reach_session(caller, &params.session).await?;
                let title = self.question_title(&params.session, params.request).await?;
                let text = answer_text(&params, title.as_deref(), &answer);
                let target = target(Some(params.session.clone()), workspace, Some(text));
                let AnswerParams {
                    session, request, ..
                } = params;
                let call = CheckedCall::Answer {
                    session,
                    request,
                    answer,
                    by,
                };
                (call, target)
            }
        };
        Ok(Checked { call, target })
    }

    /// The title of question `request` of `session`, as `wait` reports the
    /// session's open inputs; `None` when it is not an open question, which
    /// the daemon refuses an answer to.
    async fn question_title(
        &self,
        session: &SessionId,
        request: InputRequestId,
    ) -> Result<Option<String>, ControlError> {
        let now = self.now(session).await?;
        Ok(now
            .inputs
            .into_iter()
            .find(|input| input.id == request)
            .and_then(|input| match input.ask {
                Ask::Question(question) => Some(question.title),
                // An approval, a plan approval, or a kind a newer protocol
                // adds: none is a question an answer may name.
                _ => None,
            }))
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

/// What an answer sends, for an approval to show and remember: the request
/// and session it answers, with the question's `title` when known, then
/// each question's id and its answers, as `color: Blue; size: S, M`, or
/// that it declines. Naming the request keeps an approval for this answer
/// from covering the same answers to a later question.
fn answer_text(params: &AnswerParams, title: Option<&str>, answer: &QuestionAnswer) -> String {
    let asked = match title {
        Some(title) => format!(
            "request {} of session {} ({title:?})",
            params.request, params.session
        ),
        None => format!("request {} of session {}", params.request, params.session),
    };
    let answers = match answer {
        QuestionAnswer::Answers { answers } => answers
            .iter()
            .map(|(question, values)| format!("{question}: {}", values.join(", ")))
            .collect::<Vec<_>>()
            .join("; "),
        QuestionAnswer::Decline => "decline".to_owned(),
        // A kind a newer protocol adds, which this core never builds.
        _ => "answer".to_owned(),
    };
    format!("{asked}: {answers}")
}
