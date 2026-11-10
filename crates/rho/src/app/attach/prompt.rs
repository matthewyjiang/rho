//! Answering one of a session's input requests on a terminal, a line at a
//! time (#1669): an approval or a plan approval is a pick from numbered
//! options; questions are asked one after another and sent as one answer.

use std::collections::BTreeMap;

use rho_protocol::{
    Ask, DefaultStyle, InputAnswer, InputRequest, InputRequestId, Question, QuestionAsk, Selection,
};

use super::console::one_line;
use crate::{
    control_chars::{escape_controls, LineBreaks},
    permission::PermissionMode,
};

/// One input request being answered.
pub(super) struct Prompt {
    id: InputRequestId,
    kind: Kind,
}

enum Kind {
    /// One of numbered options: an approval's three, or a plan approval's
    /// permission modes, then keeping on planning.
    Pick {
        /// What the options decide on, shown above them: a plan.
        preface: Option<String>,
        options: Vec<(String, InputAnswer)>,
    },
    /// Questions asked in turn and answered together.
    Questions {
        ask: QuestionAsk,
        /// Answers to the questions before `index`.
        answers: BTreeMap<String, Vec<String>>,
        /// The question being asked.
        index: usize,
    },
}

/// What a line typed at a prompt comes to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Reply {
    /// The request's answer, to send.
    Answer(InputAnswer),
    /// The line answered a question; the next one is asked.
    Next,
    /// The line did not answer, for this reason; the prompt asks again.
    Retry(String),
}

impl Prompt {
    /// Starts answering `request`; `None` when this rho cannot answer it,
    /// as with an ask a newer daemon sends.
    pub(super) fn new(request: &InputRequest) -> Option<Self> {
        let kind = match &request.ask {
            Ask::Approval(_) => Kind::Pick {
                preface: None,
                options: vec![
                    ("allow once".to_owned(), InputAnswer::AllowOnce),
                    ("allow for session".to_owned(), InputAnswer::AllowForSession),
                    ("deny".to_owned(), InputAnswer::Deny { reason: None }),
                ],
            },
            Ask::PlanApproval(plan) => {
                // A mode this rho does not know is left out: it could not
                // name it.
                let mut options: Vec<_> = plan
                    .modes
                    .iter()
                    .filter_map(|&wire| {
                        let mode = PermissionMode::try_from(wire).ok()?;
                        Some((
                            format!("approve in {}", mode.as_str()),
                            InputAnswer::ApprovePlan {
                                mode: wire,
                                feedback: None,
                            },
                        ))
                    })
                    .collect();
                options.push((
                    "keep planning".to_owned(),
                    InputAnswer::KeepPlanning { feedback: None },
                ));
                Kind::Pick {
                    preface: Some(escape_controls(plan.plan.trim_end(), LineBreaks::Keep)),
                    options,
                }
            }
            Ask::Question(ask) if !ask.questions.is_empty() => Kind::Questions {
                ask: ask.clone(),
                answers: BTreeMap::new(),
                index: 0,
            },
            // An empty question request, or an ask a newer daemon sends:
            // `Ask` is non-exhaustive.
            _ => return None,
        };
        Some(Self {
            id: request.id,
            kind,
        })
    }

    pub(super) fn id(&self) -> InputRequestId {
        self.id
    }

    /// What the prompt shows first: a plan approval's plan, then what it
    /// asks.
    pub(super) fn opening(&self) -> String {
        match &self.kind {
            Kind::Pick {
                preface: Some(preface),
                ..
            } => format!("{preface}\n{}", self.asking()),
            Kind::Pick { preface: None, .. } | Kind::Questions { .. } => self.asking(),
        }
    }

    /// What the prompt asks now: its options, or the question being asked.
    pub(super) fn asking(&self) -> String {
        match &self.kind {
            Kind::Pick { options, .. } => {
                let options: Vec<String> = (1..)
                    .zip(options)
                    .map(|(n, (label, _))| format!("{n} {label}"))
                    .collect();
                format!("  {}\n", options.join(" · "))
            }
            Kind::Questions { ask, index, .. } => question_text(ask, *index),
        }
    }

    /// Takes a line typed at the prompt. A prompt that replied
    /// [`Reply::Answer`] is done: it takes no more lines.
    pub(super) fn reply(&mut self, line: &str) -> Reply {
        let line = line.trim();
        match &mut self.kind {
            Kind::Pick { options, .. } => match line
                .parse::<usize>()
                .ok()
                .and_then(|n| options.get(n.checked_sub(1)?))
            {
                Some((_, answer)) => Reply::Answer(answer.clone()),
                None => Reply::Retry(format!("answer a number from 1 to {}", options.len())),
            },
            Kind::Questions {
                ask,
                answers,
                index,
            } => {
                match line {
                    "/decline" => return Reply::Answer(InputAnswer::Decline),
                    "/dismiss" => return Reply::Answer(InputAnswer::Dismiss),
                    _ => {}
                }
                let question = &ask.questions[*index];
                match picked(question, line) {
                    Ok(Some(values)) => {
                        answers.insert(question.id.clone(), values);
                    }
                    Ok(None) => {}
                    Err(why) => return Reply::Retry(why),
                }
                *index += 1;
                if *index < ask.questions.len() {
                    Reply::Next
                } else {
                    Reply::Answer(InputAnswer::Answers {
                        answers: std::mem::take(answers),
                    })
                }
            }
        }
    }
}

/// Whether `question` asks only for the user's own text: the questionnaire
/// tool and MCP forms send a text field as one placeholder choice that
/// allows another answer, as ACP reads it too (`acp::elicitation::field`).
fn text_only(question: &Question) -> bool {
    question.allow_other && matches!(question.choices.as_slice(), [only] if only.value == "other")
}

/// What `line` answers `question` with: its values, or `None` to leave it
/// unanswered; `Err` says why the line does not answer it. Where the
/// question allows the user's own answer, a line that is not a pick of its
/// choices is that answer.
fn picked(question: &Question, line: &str) -> Result<Option<Vec<String>>, String> {
    if line.is_empty() {
        return match &question.default {
            Some(default) if default.style == DefaultStyle::Selected => {
                Ok(Some(distinct(default.values.iter())))
            }
            _ if question.required => Err("this question needs an answer".to_owned()),
            _ => Ok(None),
        };
    }
    if text_only(question) {
        return Ok(Some(vec![line.to_owned()]));
    }
    let numbers: Option<Vec<usize>> = line
        .split([',', ' '])
        .filter(|part| !part.is_empty())
        .map(|part| part.parse().ok())
        .collect();
    let choice = |n: usize| question.choices.get(n.checked_sub(1)?);
    match numbers {
        Some(numbers) if !numbers.is_empty() && numbers.iter().all(|&n| choice(n).is_some()) => {
            if numbers.len() > 1 && question.selection != Selection::Many {
                return Err("pick one".to_owned());
            }
            Ok(Some(distinct(
                numbers.into_iter().filter_map(choice).map(|c| &c.value),
            )))
        }
        _ if question.allow_other => Ok(Some(vec![line.to_owned()])),
        Some(numbers) => match numbers.into_iter().find(|&n| choice(n).is_none()) {
            Some(missing) => Err(format!("there is no choice {missing}")),
            None => Err("pick by number".to_owned()),
        },
        None => Err("pick by number".to_owned()),
    }
}

/// `values` without repeats, in order.
fn distinct<'a>(values: impl Iterator<Item = &'a String>) -> Vec<String> {
    let mut distinct = Vec::new();
    for value in values {
        if !distinct.contains(value) {
            distinct.push(value.clone());
        }
    }
    distinct
}

/// Question `index` of `ask`, its choices, and how to answer it.
fn question_text(ask: &QuestionAsk, index: usize) -> String {
    let question = &ask.questions[index];
    let mut text = format!("  {}/{} ", index + 1, ask.questions.len());
    if let Some(header) = &question.header {
        text.push_str(&format!("{}: ", one_line(header)));
    }
    text.push_str(&one_line(&question.prompt));
    if !question.required {
        text.push_str(" (optional)");
    }
    text.push('\n');
    if let Some(help) = &question.help {
        text.push_str(&format!("  {}\n", one_line(help)));
    }
    let mut how = Vec::new();
    if text_only(question) {
        how.push("type your answer".to_owned());
    } else {
        for (n, choice) in (1..).zip(&question.choices) {
            text.push_str(&format!("    {n} {}", one_line(&choice.label)));
            if let Some(description) = &choice.description {
                text.push_str(&format!(" — {}", one_line(description)));
            }
            text.push('\n');
        }
        match (question.choices.is_empty(), question.selection) {
            (true, _) => {}
            (false, Selection::Many) => how.push("pick any, like 1,3".to_owned()),
            // `Selection` is non-exhaustive; another is answered as one.
            (false, _) => how.push("pick one".to_owned()),
        }
        if question.allow_other {
            how.push("or type your own answer".to_owned());
        }
    }
    match &question.default {
        Some(default) if default.style == DefaultStyle::Selected => {
            how.push(format!("Enter takes {}", labels(question, &default.values)));
        }
        Some(default) => {
            how.push(format!("suggested: {}", labels(question, &default.values)));
            if !question.required {
                how.push("Enter skips".to_owned());
            }
        }
        None if !question.required => how.push("Enter skips".to_owned()),
        None => {}
    }
    how.push("/decline or /dismiss closes the questions".to_owned());
    text.push_str(&format!("  {}\n", how.join("; ")));
    text
}

/// `values` as their choices' labels, or as typed where no choice has one,
/// without repeats.
fn labels(question: &Question, values: &[String]) -> String {
    distinct(values.iter())
        .iter()
        .map(|value| {
            question
                .choices
                .iter()
                .find(|choice| &choice.value == value)
                .map_or(value.as_str(), |choice| choice.label.as_str())
        })
        .map(one_line)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
#[path = "prompt_tests.rs"]
mod tests;
