//! A call in two steps: [`SessionControl::check`] parses its arguments and
//! applies every policy that does not depend on what else runs (the access
//! the action needs, the workspaces the grant reaches, the nesting depth),
//! and [`SessionControl::run`] admits it under the per-holder session cap
//! and acts. A front end that asks a person first, as the native `sessions`
//! tool does, checks before asking, so nobody approves a call the core would
//! refuse, and names the [`Target`] the check found.

use std::path::PathBuf;

use rho_protocol::{Ask, InputRequest, InputRequestId, QuestionAnswer, QuestionAsk, SessionId};
use serde_json::{Map, Value};

use super::{
    answer::not_open, canonical, dispatch::parse_arguments, prompt_content, Action, AnswerParams,
    Caller, CancelParams, ControlError, ListParams, NewParams, PromptParams, ReadParams, Refusal,
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
    /// answer gives answers or declines, from a calling session, to a
    /// question the session has open. Whether that session may answer is
    /// the daemon's to decide, when it answers, and so is whether only the
    /// user may answer the question, which its wire ask does not say.
    ///
    /// An answer's check reads the asking session's open inputs as `wait`
    /// does, on a connection of its own that replays the session's events
    /// from its start with no deadline; a failure there fails the check, so
    /// nothing is sent.
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
                prompt_content(&params.prompt)?;
                self.check_depth(caller)?;
                let target = target(
                    Some(params.session.clone()),
                    workspace,
                    Some(params.prompt.clone()),
                );
                (CheckedCall::Prompt(params), target)
            }
            Action::Steer => {
                let params: SteerParams = parse_arguments(action.name(), arguments)?;
                let workspace = self.reach_session(caller, &params.session).await?;
                let target = target(
                    Some(params.session.clone()),
                    workspace,
                    Some(params.prompt.clone()),
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
                let question = self.open_question(&params.session, params.request).await?;
                let text = answer_text(&params, &question, &answer);
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

    /// Question `request` of `session`, from the session's open inputs as
    /// `wait` reports them, its subagents' included. Refuses, as the daemon
    /// would, a request that is not open or is not a question, so nobody is
    /// asked to approve an answer the daemon refuses.
    async fn open_question(
        &self,
        session: &SessionId,
        request: InputRequestId,
    ) -> Result<QuestionAsk, ControlError> {
        let now = self.now(session).await?;
        find_question(now.inputs, request).map_err(|refused| match refused {
            Unanswerable::NotOpen => not_open(session, request),
            Unanswerable::NotQuestion => ControlError::Invalid(format!(
                "input request {request} of session {session} is not a question: only a person answers an approval or a plan approval"
            )),
        })
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

/// Why the check refuses an answer before anyone approves it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Unanswerable {
    /// No open request has the id: it was answered, timed out, or
    /// withdrawn, or never opened.
    NotOpen,
    /// The request is an approval or a plan approval, which only a person
    /// answers.
    NotQuestion,
}

/// The question that request `request` among the open `inputs` asks.
fn find_question(
    inputs: Vec<InputRequest>,
    request: InputRequestId,
) -> Result<QuestionAsk, Unanswerable> {
    let input = inputs
        .into_iter()
        .find(|input| input.id == request)
        .ok_or(Unanswerable::NotOpen)?;
    match input.ask {
        Ask::Question(question) => Ok(question),
        // An approval, a plan approval, or a kind a newer protocol adds:
        // none is a question an answer may name.
        _ => Err(Unanswerable::NotQuestion),
    }
}

/// What an answer sends, for an approval to show and remember: the request
/// and session it answers with the `question`'s title, then each question
/// it answers by id and prompt with its answers, each quoted, as
/// `color ("Which color?"): ["Blue"]; size ("Which sizes?"): ["S", "M"]`,
/// or the questions it declines. The quoting keeps two answers apart from
/// one answer that holds a comma, since an approval is remembered by this
/// text. Request ids restart when the daemon reloads the
/// session, and titles default to the same text, so the prompts are what
/// keep an approval for this answer from covering the same answers to
/// another question.
fn answer_text(params: &AnswerParams, question: &QuestionAsk, answer: &QuestionAnswer) -> String {
    // A question id the ask lacks, which the daemon refuses, shows bare.
    let named = |id: &str| match question.questions.iter().find(|asked| asked.id == id) {
        Some(asked) => format!("{id} ({:?})", asked.prompt),
        None => id.to_owned(),
    };
    let answers = match answer {
        QuestionAnswer::Answers { answers } => answers
            .iter()
            .map(|(id, values)| format!("{}: {values:?}", named(id)))
            .collect::<Vec<_>>()
            .join("; "),
        QuestionAnswer::Decline => format!(
            "decline {}",
            question
                .questions
                .iter()
                .map(|asked| named(&asked.id))
                .collect::<Vec<_>>()
                .join("; ")
        ),
        // A kind a newer protocol adds, which this core never builds.
        _ => "answer".to_owned(),
    };
    format!(
        "request {} of session {} ({:?}): {answers}",
        params.request, params.session, question.title
    )
}

#[cfg(test)]
#[path = "check_tests.rs"]
mod tests;
