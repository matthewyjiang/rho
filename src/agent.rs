use std::{future::Future, pin::Pin};

use thiserror::Error;

mod compaction;
mod context_tracker;
mod history;
pub mod questionnaire;

pub use compaction::CompactionConfig;
pub use history::{HistorySink, SessionHistorySink};
pub use questionnaire::{
    QuestionnaireAnswer, QuestionnaireQuestion, QuestionnaireQuestionKind, QuestionnaireRequest,
    QuestionnaireResponse,
};

use compaction::{
    build_summary_request_messages, partition_messages_for_compaction,
    replacement_history_from_summary, should_compact,
};
use context_tracker::ContextTracker;

use crate::model::{
    context::estimate_context_tokens, openai::prompt_cache_key_from_session_id, ContentBlock,
    ContextUsage, DynModelProvider, Message, ModelError, ModelEvent, ModelRequest, ModelResponse,
    ModelUsage,
};
use crate::prompt::system_prompt;
use crate::tool::{truncate, ToolContext, ToolDisplayStyle, ToolError, ToolRegistry, ToolResult};

pub type QuestionnaireFuture =
    Pin<Box<dyn Future<Output = Result<QuestionnaireResponse, AgentError>> + Send>>;
pub type QuestionnaireHandler<'a> = &'a mut dyn FnMut(QuestionnaireRequest) -> QuestionnaireFuture;

#[derive(Debug, Error)]
pub enum AgentError {
    #[error("Provider error: {0}")]
    Provider(#[from] ModelError),
    #[error("Tool error: {0}")]
    Tool(#[from] ToolError),
    #[error("Message persistence error: {0}")]
    MessagePersistence(#[from] anyhow::Error),
    #[error("Questionnaire error: {0}")]
    Questionnaire(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentEvent {
    StepStarted(usize),
    ToolStarted {
        name: String,
        command: Option<String>,
        display_style: ToolDisplayStyle,
        display_lines: Vec<String>,
    },
    OutputDelta(String),
    ReasoningDelta(String),
    ContextUsage(ContextUsage),
    Usage(ModelUsage),
    ToolUpdated {
        display_lines: Vec<String>,
    },
    ToolFinished {
        name: String,
        command: Option<String>,
        ok: bool,
        content: String,
        display_style: ToolDisplayStyle,
        display_lines: Vec<String>,
    },
    QuestionnaireStarted(QuestionnaireRequest),
    QuestionnaireFinished(QuestionnaireResponse),
}

enum CompactionTrigger {
    Automatic,
    Manual,
}

const MAX_INVALID_RESPONSE_RETRIES: usize = 1;

pub struct Agent {
    provider: DynModelProvider,
    tools: ToolRegistry,
    ctx: ToolContext,
    messages: Vec<Message>,
    history_sink: Option<Box<dyn HistorySink>>,
    prompt_cache_key: Option<String>,
    include_system_prompt: bool,
    compaction: CompactionConfig,
    context_tracker: ContextTracker,
}

impl Agent {
    pub fn new(provider: DynModelProvider, tools: ToolRegistry, ctx: ToolContext) -> Self {
        let messages = initial_messages(&tools, &ctx.cwd, true);
        Self {
            provider,
            tools,
            ctx,
            messages,
            history_sink: None,
            prompt_cache_key: None,
            include_system_prompt: true,
            compaction: CompactionConfig::default(),
            context_tracker: ContextTracker::default(),
        }
    }

    pub fn without_system_prompt(mut self) -> Self {
        self.include_system_prompt = false;
        self.messages = initial_messages(&self.tools, &self.ctx.cwd, self.include_system_prompt);
        self
    }

    pub fn with_history(mut self, history: Vec<Message>) -> Self {
        self.messages.extend(history);
        self
    }

    pub fn replace_history(&mut self, history: Vec<Message>) {
        self.messages = initial_messages(&self.tools, &self.ctx.cwd, self.include_system_prompt);
        self.messages.extend(history);
        self.context_tracker.history_replaced();
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn set_history_sink(&mut self, sink: impl HistorySink + 'static) {
        self.history_sink = Some(Box::new(sink));
    }

    pub fn clear_history_sink(&mut self) {
        self.history_sink = None;
    }

    pub fn set_compaction_config(&mut self, compaction: CompactionConfig) {
        self.compaction = compaction;
    }

    pub fn set_context_window(&mut self, context_window: Option<u64>) {
        self.context_tracker.set_configured_window(context_window);
    }

    pub fn set_session_id(&mut self, session_id: Option<String>) {
        self.prompt_cache_key = session_id
            .as_deref()
            .and_then(prompt_cache_key_from_session_id);
    }

    pub fn replace_provider(&mut self, provider: DynModelProvider) {
        self.provider = provider;
        self.context_tracker.replace_provider();
    }

    pub fn set_provider_reasoning(&mut self, reasoning: crate::reasoning::ReasoningLevel) -> bool {
        if self.provider.set_reasoning(reasoning) {
            self.context_tracker.replace_provider();
            true
        } else {
            false
        }
    }

    pub fn reset(&mut self) {
        self.messages = initial_messages(&self.tools, &self.ctx.cwd, self.include_system_prompt);
        self.context_tracker.reset();
    }

    pub async fn compact(
        &mut self,
        on_event: impl FnMut(AgentEvent) -> Result<(), ModelError>,
    ) -> Result<bool, AgentError> {
        let specs = self.tools.specs();
        self.compact_history(&specs, CompactionTrigger::Manual, on_event)
            .await
    }

    pub async fn run(&mut self, user_prompt: String) -> Result<String, AgentError> {
        self.run_with_events(user_prompt, |_| Ok(())).await
    }

    pub fn load_skill(&mut self, skill: &crate::skills::Skill) -> Result<(), AgentError> {
        self.push_message(Message::user_text(format!(
            "Loaded skill `{}` from {}:\n\n{}",
            skill.name,
            skill.path.display(),
            truncate(skill.contents.clone(), self.ctx.max_output_bytes)
        )))
    }

    fn push_message(&mut self, message: Message) -> Result<(), AgentError> {
        if let Some(sink) = &mut self.history_sink {
            sink.append_message(&message)?;
        }
        self.messages.push(message);
        Ok(())
    }

    pub async fn run_with_events(
        &mut self,
        user_prompt: String,
        on_event: impl FnMut(AgentEvent) -> Result<(), ModelError>,
    ) -> Result<String, AgentError> {
        self.run_with_events_and_steering(user_prompt, on_event, || Ok(None))
            .await
    }

    pub async fn run_with_events_and_steering(
        &mut self,
        user_prompt: String,
        on_event: impl FnMut(AgentEvent) -> Result<(), ModelError>,
        next_steer: impl FnMut() -> Result<Option<String>, AgentError>,
    ) -> Result<String, AgentError> {
        self.run_with_content_and_events_and_steering(
            vec![ContentBlock::Text(user_prompt)],
            on_event,
            next_steer,
        )
        .await
    }

    pub async fn run_with_content_and_events_and_steering(
        &mut self,
        user_content: Vec<ContentBlock>,
        on_event: impl FnMut(AgentEvent) -> Result<(), ModelError>,
        next_steer: impl FnMut() -> Result<Option<String>, AgentError>,
    ) -> Result<String, AgentError> {
        self.run_with_content_and_events_questionnaire_and_steering(
            user_content,
            on_event,
            None,
            next_steer,
        )
        .await
    }

    pub async fn run_with_content_and_events_questionnaire_and_steering(
        &mut self,
        user_content: Vec<ContentBlock>,
        mut on_event: impl FnMut(AgentEvent) -> Result<(), ModelError>,
        mut ask_questionnaire: Option<QuestionnaireHandler<'_>>,
        mut next_steer: impl FnMut() -> Result<Option<String>, AgentError>,
    ) -> Result<String, AgentError> {
        let mut specs = self.tools.specs();
        if ask_questionnaire.is_some() {
            specs.push(questionnaire::tool_spec());
        }
        self.push_message(Message::User(user_content))?;

        let mut step = 1usize;
        let mut invalid_response_retries = 0usize;
        loop {
            self.compact_history(&specs, CompactionTrigger::Automatic, &mut on_event)
                .await?;
            on_event(AgentEvent::StepStarted(step))?;
            if let Some(context_usage) = self
                .context_tracker
                .before_provider_request(&self.messages, &specs)
            {
                on_event(AgentEvent::ContextUsage(context_usage))?;
            }
            let request_estimated_tokens = estimate_context_tokens(&self.messages, &specs);
            let response = match self
                .provider
                .send_turn_stream(
                    ModelRequest {
                        messages: self.messages.clone(),
                        tools: specs.clone(),
                        prompt_cache_key: self.prompt_cache_key.clone(),
                    },
                    &mut |event| match event {
                        ModelEvent::OutputDelta(text) => on_event(AgentEvent::OutputDelta(text)),
                        ModelEvent::ReasoningDelta(text) => {
                            on_event(AgentEvent::ReasoningDelta(text))
                        }
                        ModelEvent::WebSearch(detail) => on_event(AgentEvent::ToolFinished {
                            name: "web_search".into(),
                            command: None,
                            ok: true,
                            content: detail.clone(),
                            display_style: ToolDisplayStyle::default_tool(),
                            display_lines: vec![format!("web search: {detail}")],
                        }),
                        ModelEvent::Usage(usage) => {
                            if let Some(context_usage) = self
                                .context_tracker
                                .record_provider_usage(&usage, request_estimated_tokens)
                            {
                                on_event(AgentEvent::ContextUsage(context_usage))?;
                            }
                            on_event(AgentEvent::Usage(usage))
                        }
                    },
                )
                .await
            {
                Ok(response) => {
                    invalid_response_retries = 0;
                    response
                }
                Err(ModelError::Interrupted) => return Err(ModelError::Interrupted.into()),
                Err(err)
                    if should_retry_model_error(&err)
                        && invalid_response_retries < MAX_INVALID_RESPONSE_RETRIES =>
                {
                    invalid_response_retries += 1;
                    self.push_message(Message::user_text(format!(
                        "The previous assistant response could not be processed by the client. Error: {err}\n\nPlease continue from the last request. If you attempted a tool call, emit valid tool-call JSON that exactly matches the required schema."
                    )))?;
                    step += 1;
                    continue;
                }
                Err(err) => return Err(err.into()),
            };
            match response {
                ModelResponse::Assistant(blocks) => {
                    let tool_calls: Vec<_> = blocks
                        .iter()
                        .filter_map(|block| match block {
                            ContentBlock::ToolCall(call) => Some(call.clone()),
                            ContentBlock::Text(_) | ContentBlock::Image(_) => None,
                        })
                        .collect();
                    if tool_calls.is_empty() {
                        let answer = blocks
                            .into_iter()
                            .filter_map(|block| match block {
                                ContentBlock::Text(text) => Some(text),
                                ContentBlock::Image(_) | ContentBlock::ToolCall(_) => None,
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                        self.push_message(Message::assistant_text(answer.clone()))?;
                        let Some(steer) = next_steer()? else {
                            return Ok(answer);
                        };
                        self.push_message(Message::user_text(steer))?;
                    } else {
                        let mut pending_assistant_message = Some(Message::Assistant(blocks));
                        let mut deferred_interrupt = None;
                        for call in tool_calls.iter().cloned() {
                            let name = call.name.clone();
                            let is_questionnaire = name == questionnaire::TOOL_NAME;
                            let tool = (!is_questionnaire)
                                .then(|| self.tools.get(&call.name))
                                .flatten();
                            let questionnaire_request = if is_questionnaire {
                                Some(questionnaire::parse_request(call.arguments.clone()))
                            } else {
                                None
                            };
                            let (display_style, command, start_display_lines) =
                                match (&tool, &questionnaire_request) {
                                    (_, Some(Ok(request))) => (
                                        ToolDisplayStyle::default_tool(),
                                        None,
                                        questionnaire::start_display_lines(request),
                                    ),
                                    (_, Some(Err(err))) => (
                                        ToolDisplayStyle::default_tool(),
                                        None,
                                        vec![questionnaire::TOOL_NAME.into(), err.clone()],
                                    ),
                                    (Some(tool), None) => {
                                        let display_style = tool.display_style();
                                        let command = tool.display_command(&call.arguments);
                                        let start_display_lines =
                                            tool.display_start_lines(&call.arguments, &self.ctx);
                                        (display_style, command, start_display_lines)
                                    }
                                    (None, None) => (
                                        ToolDisplayStyle::default_tool(),
                                        None,
                                        vec![call.name.clone()],
                                    ),
                                };
                            if deferred_interrupt.is_none() {
                                on_event(AgentEvent::ToolStarted {
                                    name: name.clone(),
                                    command: command.clone(),
                                    display_style,
                                    display_lines: start_display_lines,
                                })?;
                            }
                            if let Some(message) = pending_assistant_message.take() {
                                self.push_message(message)?;
                            }

                            let (result, event_content, display_lines) = if let Some(parse_result) =
                                questionnaire_request
                            {
                                match parse_result {
                                    Ok(request) => {
                                        if deferred_interrupt.is_none() {
                                            match on_event(AgentEvent::QuestionnaireStarted(
                                                request.clone(),
                                            )) {
                                                Ok(()) => {}
                                                Err(ModelError::Interrupted) => {
                                                    deferred_interrupt =
                                                        Some(ModelError::Interrupted);
                                                }
                                                Err(err) => return Err(err.into()),
                                            }
                                        }
                                        let result = if deferred_interrupt.is_some() {
                                            ToolResult {
                                                id: call.id.clone(),
                                                ok: false,
                                                content: "questionnaire interrupted".into(),
                                            }
                                        } else if let Some(ask_questionnaire) =
                                            ask_questionnaire.as_mut()
                                        {
                                            match ask_questionnaire(request.clone()).await {
                                                Ok(response) => {
                                                    if deferred_interrupt.is_none() {
                                                        match on_event(
                                                            AgentEvent::QuestionnaireFinished(
                                                                response.clone(),
                                                            ),
                                                        ) {
                                                            Ok(()) => {}
                                                            Err(ModelError::Interrupted) => {
                                                                deferred_interrupt =
                                                                    Some(ModelError::Interrupted);
                                                            }
                                                            Err(err) => return Err(err.into()),
                                                        }
                                                    }
                                                    ToolResult {
                                                        id: call.id.clone(),
                                                        ok: true,
                                                        content: questionnaire::response_content(
                                                            &response,
                                                        ),
                                                    }
                                                }
                                                Err(err) => ToolResult {
                                                    id: call.id.clone(),
                                                    ok: false,
                                                    content: format!("questionnaire failed: {err}"),
                                                },
                                            }
                                        } else {
                                            ToolResult {
                                                id: call.id.clone(),
                                                ok: false,
                                                content:
                                                    "questionnaire is unavailable in this mode"
                                                        .into(),
                                            }
                                        };
                                        let display_lines = questionnaire::finished_display_lines(
                                            &request,
                                            &result.content,
                                        );
                                        (result, None, display_lines)
                                    }
                                    Err(err) => {
                                        let result = ToolResult {
                                            id: call.id.clone(),
                                            ok: false,
                                            content: err,
                                        };
                                        let display_lines = vec![
                                            questionnaire::TOOL_NAME.into(),
                                            result.content.clone(),
                                        ];
                                        (result, None, display_lines)
                                    }
                                }
                            } else {
                                match tool {
                                    Some(tool) => {
                                        let event_content =
                                            tool.display_content(&call.arguments, &self.ctx);
                                        let execution_tool = tool.clone();
                                        let args = call.arguments.clone();
                                        let ctx = self.ctx.clone();
                                        let id = call.id.clone();
                                        let (progress_tx, mut progress_rx) =
                                            tokio::sync::mpsc::unbounded_channel();
                                        let mut task = tokio::spawn(async move {
                                            let mut on_update = move |display_lines| {
                                                let _ = progress_tx.send(display_lines);
                                            };
                                            execution_tool
                                                .call_with_updates(args, ctx, id, &mut on_update)
                                                .await
                                        });
                                        let result = loop {
                                            tokio::select! {
                                                Some(display_lines) = progress_rx.recv() => {
                                                    if deferred_interrupt.is_none() {
                                                        match on_event(AgentEvent::ToolUpdated { display_lines }) {
                                                            Ok(()) => {}
                                                            Err(ModelError::Interrupted) => {
                                                                deferred_interrupt = Some(ModelError::Interrupted);
                                                            }
                                                            Err(err) => return Err(err.into()),
                                                        }
                                                    }
                                                }
                                                joined = &mut task => {
                                                    break match joined {
                                                        Ok(Ok(result)) => result,
                                                        Ok(Err(err)) => ToolResult {
                                                            id: call.id.clone(),
                                                            ok: false,
                                                            content: err.to_string(),
                                                        },
                                                        Err(err) => ToolResult {
                                                            id: call.id.clone(),
                                                            ok: false,
                                                            content: format!("tool task failed: {err}"),
                                                        },
                                                    };
                                                }
                                            }
                                        };
                                        let mut display_lines =
                                            tool.display_lines(&call.arguments, &self.ctx, &result);
                                        if !result.ok
                                            && !display_lines
                                                .iter()
                                                .any(|line| line == &result.content)
                                        {
                                            display_lines.push(result.content.clone());
                                        }
                                        (result, event_content, display_lines)
                                    }
                                    None => {
                                        let result = ToolResult {
                                            id: call.id.clone(),
                                            ok: false,
                                            content: format!("Unknown tool: {}", call.name),
                                        };
                                        let display_lines =
                                            vec![call.name.clone(), result.content.clone()];
                                        (result, None, display_lines)
                                    }
                                }
                            };
                            let display_content =
                                event_content.unwrap_or_else(|| result.content.clone());
                            let ok = result.ok;
                            self.push_message(Message::ToolResult(result))?;
                            if deferred_interrupt.is_none() {
                                match on_event(AgentEvent::ToolFinished {
                                    name,
                                    command,
                                    ok,
                                    content: display_content,
                                    display_style,
                                    display_lines,
                                }) {
                                    Ok(()) => {}
                                    Err(ModelError::Interrupted) => {
                                        deferred_interrupt = Some(ModelError::Interrupted);
                                    }
                                    Err(err) => return Err(err.into()),
                                }
                            }
                        }
                        if let Some(err) = deferred_interrupt {
                            return Err(err.into());
                        }
                    }
                }
            }
            step += 1;
        }
    }

    async fn compact_history(
        &mut self,
        specs: &[crate::tool::ToolSpec],
        trigger: CompactionTrigger,
        mut on_event: impl FnMut(AgentEvent) -> Result<(), ModelError>,
    ) -> Result<bool, AgentError> {
        let estimate = self
            .context_tracker
            .estimate_for_compaction(&self.messages, specs);
        if matches!(trigger, CompactionTrigger::Automatic)
            && !should_compact(&self.compaction, estimate.tokens, estimate.context_window)
        {
            return Ok(false);
        }
        let Some(context_window) = estimate.context_window.filter(|window| *window > 0) else {
            return Ok(false);
        };
        let target_tokens = self.compaction.target_tokens(context_window);
        let Some(partition) =
            partition_messages_for_compaction(&self.messages, specs, target_tokens)
        else {
            return Ok(false);
        };

        let response = match self
            .provider
            .send_turn_stream(
                ModelRequest {
                    messages: build_summary_request_messages(&partition.compacted_messages),
                    tools: Vec::new(),
                    prompt_cache_key: self.prompt_cache_key.clone(),
                },
                &mut |event| match event {
                    ModelEvent::OutputDelta(_)
                    | ModelEvent::ReasoningDelta(_)
                    | ModelEvent::WebSearch(_) => Ok(()),
                    ModelEvent::Usage(usage) => on_event(AgentEvent::Usage(usage)),
                },
            )
            .await
        {
            Ok(response) => response,
            Err(ModelError::Interrupted) => return Err(ModelError::Interrupted.into()),
            // Automatic compaction is best effort because its threshold leaves
            // headroom for the normal request. Manual compaction reports failure.
            Err(err) => {
                return match trigger {
                    CompactionTrigger::Automatic => Ok(false),
                    CompactionTrigger::Manual => Err(err.into()),
                };
            }
        };
        let ModelResponse::Assistant(blocks) = response;
        let summary = blocks
            .into_iter()
            .filter_map(|block| match block {
                ContentBlock::Text(text) => Some(text),
                ContentBlock::Image(_) | ContentBlock::ToolCall(_) => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string();
        if summary.is_empty() {
            return Ok(false);
        }

        self.messages = replacement_history_from_summary(partition, summary);
        self.persist_history_replacement()?;
        let context_usage = self.context_tracker.record_compaction();
        on_event(AgentEvent::ContextUsage(context_usage))?;
        Ok(true)
    }

    fn persist_history_replacement(&mut self) -> Result<(), AgentError> {
        if let Some(sink) = &mut self.history_sink {
            let first_history_index = self
                .messages
                .iter()
                .position(|message| !matches!(message, Message::System(_)))
                .unwrap_or(self.messages.len());
            sink.replace_history(&self.messages[first_history_index..])?;
        }
        Ok(())
    }
}

fn should_retry_model_error(error: &ModelError) -> bool {
    matches!(error, ModelError::InvalidResponse(_))
}

fn initial_messages(
    tools: &ToolRegistry,
    cwd: &std::path::Path,
    include_system_prompt: bool,
) -> Vec<Message> {
    if include_system_prompt {
        vec![Message::System(system_prompt(&tools.specs(), cwd))]
    } else {
        Vec::new()
    }
}

#[cfg(test)]
#[path = "agent_tests.rs"]
mod tests;
