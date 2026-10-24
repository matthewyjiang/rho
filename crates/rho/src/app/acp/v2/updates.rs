//! Rho's v1 `session/update` notifications in ACP v2 form.
//!
//! The session host emits v1 updates; a v2 connection rewrites each one here
//! before it reaches the wire. Shapes that only gained optional fields or open
//! enums in v2 cross through [`bridge`]; everything v2 redesigned is mapped
//! field by field:
//!
//! - `tool_call` becomes the first `tool_call_update` for its ID, which v2
//!   treats as the creating upsert.
//! - A v1 update field that is absent means "unchanged", which is v2's
//!   undefined, never its `null` (clear).
//! - `plan` becomes `plan_update` for Rho's single todo plan.
//! - `current_mode_update` is dropped: the mode is a config option in v2, and
//!   every mode change also refreshes `config_option_update`.
//! - A v1 diff of complete file texts becomes structured `changes` plus a
//!   `git_patch`.

use agent_client_protocol::schema::{v1, v2, MaybeUndefined};
use agent_client_protocol::Error as AcpError;

use super::{bridge, git_patch::FileContents};

/// The ID of Rho's one plan: its todo list.
const TODO_PLAN_ID: &str = "rho-todos";

/// The v2 notification for `notification`, or `None` when v2 has no
/// counterpart.
pub(super) fn notification(
    notification: v1::SessionNotification,
) -> Result<Option<v2::UpdateSessionNotification>, AcpError> {
    let v1::SessionNotification {
        session_id,
        update,
        meta,
        ..
    } = notification;
    let Some(update) = session_update(update)? else {
        return Ok(None);
    };
    Ok(Some(
        v2::UpdateSessionNotification::new(bridge::<_, v2::SessionId>(&session_id)?, update)
            .meta(meta),
    ))
}

fn session_update(update: v1::SessionUpdate) -> Result<Option<v2::SessionUpdate>, AcpError> {
    use v1::SessionUpdate as V1;
    use v2::SessionUpdate as V2;
    Ok(Some(match update {
        V1::UserMessageChunk(chunk) => V2::UserMessageChunk(content_chunk(chunk)?),
        V1::AgentMessageChunk(chunk) => V2::AgentMessageChunk(content_chunk(chunk)?),
        V1::AgentThoughtChunk(chunk) => V2::AgentThoughtChunk(content_chunk(chunk)?),
        V1::ToolCall(call) => V2::ToolCallUpdate(created_tool_call(call)?),
        V1::ToolCallUpdate(update) => V2::ToolCallUpdate(tool_call_update(update)?),
        V1::Plan(plan) => V2::PlanUpdate(
            v2::PlanUpdate::new(v2::PlanUpdateContent::Items(v2::PlanItems::new(
                TODO_PLAN_ID,
                bridge(&plan.entries)?,
            )))
            .meta(plan.meta),
        ),
        V1::AvailableCommandsUpdate(update) => V2::AvailableCommandsUpdate(
            v2::AvailableCommandsUpdate::new(available_commands(update.available_commands)?)
                .meta(update.meta),
        ),
        V1::CurrentModeUpdate(_) => return Ok(None),
        V1::ConfigOptionUpdate(update) => V2::ConfigOptionUpdate(
            v2::ConfigOptionUpdate::new(config_options(update.config_options)?).meta(update.meta),
        ),
        V1::SessionInfoUpdate(update) => V2::SessionInfoUpdate(bridge(&update)?),
        V1::UsageUpdate(update) => V2::UsageUpdate(bridge(&update)?),
        V1::Notice(notice) => V2::Notice(bridge(&notice)?),
        V1::CompactionUpdate(update) => V2::CompactionUpdate(bridge(&update)?),
        V1::CompactionSummaryChunk(chunk) => V2::CompactionSummaryChunk(bridge(&chunk)?),
        _ => return Err(AcpError::internal_error().data("no ACP v2 form for this session update")),
    }))
}

/// Rho names every message chunk it sends; v2 requires the name.
fn content_chunk(chunk: v1::ContentChunk) -> Result<v2::ContentChunk, AcpError> {
    let Some(message_id) = chunk.message_id else {
        return Err(AcpError::internal_error().data("ACP v2 message chunk without a message id"));
    };
    Ok(v2::ContentChunk::new(
        bridge(&chunk.content)?,
        bridge::<_, v2::MessageId>(&message_id)?,
    )
    .meta(chunk.meta))
}

/// A v1 `tool_call` states every field, so each one is set on the creating
/// upsert.
fn created_tool_call(call: v1::ToolCall) -> Result<v2::ToolCallUpdate, AcpError> {
    let v1::ToolCall {
        tool_call_id,
        title,
        name,
        kind,
        status,
        content,
        locations,
        raw_input,
        raw_output,
        meta,
        ..
    } = call;
    let fields = v1::ToolCallUpdateFields::new()
        .title(title)
        .name(name)
        .kind(kind)
        .status(status)
        .content(content)
        .locations(locations)
        .raw_input(raw_input)
        .raw_output(raw_output);
    tool_call_update(v1::ToolCallUpdate::new(tool_call_id, fields).meta(meta))
}

pub(super) fn tool_call_update(update: v1::ToolCallUpdate) -> Result<v2::ToolCallUpdate, AcpError> {
    let v1::ToolCallUpdate {
        tool_call_id,
        fields,
        meta,
        ..
    } = update;
    let v1::ToolCallUpdateFields {
        kind,
        status,
        title,
        name,
        content,
        locations,
        raw_input,
        raw_output,
        ..
    } = fields;
    let content = content
        .map(|content| content.into_iter().map(tool_call_content).collect())
        .transpose()?;
    Ok(
        v2::ToolCallUpdate::new(bridge::<_, v2::ToolCallId>(&tool_call_id)?)
            .title(unchanged_if_absent(title))
            .name(unchanged_if_absent(name))
            .kind(unchanged_if_absent(
                kind.map(|kind| bridge(&kind)).transpose()?,
            ))
            .status(unchanged_if_absent(
                status.map(|status| bridge(&status)).transpose()?,
            ))
            .content(unchanged_if_absent(content))
            .locations(unchanged_if_absent(
                locations.map(|locations| bridge(&locations)).transpose()?,
            ))
            .raw_input(unchanged_if_absent(raw_input))
            .raw_output(unchanged_if_absent(raw_output))
            .meta(unchanged_if_absent(meta)),
    )
}

/// v1 leaves a field out to keep it; v2 does too, while `null` clears it.
fn unchanged_if_absent<T>(value: Option<T>) -> MaybeUndefined<T> {
    match value {
        Some(value) => MaybeUndefined::Value(value),
        None => MaybeUndefined::Undefined,
    }
}

fn tool_call_content(content: v1::ToolCallContent) -> Result<v2::ToolCallContent, AcpError> {
    Ok(match content {
        v1::ToolCallContent::Content(content) => v2::ToolCallContent::Content(Box::new(
            v2::Content::new(bridge::<_, v2::ContentBlock>(&content.content)?).meta(content.meta),
        )),
        v1::ToolCallContent::Diff(diff) => v2::ToolCallContent::Diff(file_diff(&diff)),
        // Client terminals are v1 only: a v2 host is never offered them.
        v1::ToolCallContent::Terminal(terminal) => {
            v2::ToolCallContent::Terminal(bridge(&terminal)?)
        }
        _ => return Err(AcpError::internal_error().data("no ACP v2 form for this tool content")),
    })
}

/// A v1 diff holds the complete old and new texts of one file, so the change
/// and its patch can be rebuilt. A deletion is marked in the diff's `_meta`,
/// since v1 shows it as an emptied file; the marker is Rho's, so it is
/// dropped here.
fn file_diff(diff: &v1::Diff) -> v2::Diff {
    let path = diff.path.as_path();
    let mut meta = diff.meta.clone();
    let deleted = meta
        .as_mut()
        .and_then(|meta| meta.remove(crate::app::acp::events::DELETED_FILE_META_KEY))
        .is_some_and(|marker| marker == true);
    let meta = meta.filter(|meta| !meta.is_empty());
    let (change, contents) = match (&diff.old_text, deleted) {
        (None, _) => (
            v2::DiffChange::add(path.to_path_buf()),
            FileContents::Created(&diff.new_text),
        ),
        (Some(old), true) => (
            v2::DiffChange::delete(path.to_path_buf()),
            FileContents::Deleted(old),
        ),
        (Some(old), false) => (
            v2::DiffChange::modify(path.to_path_buf()),
            FileContents::Modified {
                old,
                new: &diff.new_text,
            },
        ),
    };
    let patch = super::git_patch::file_patch(path, contents);
    v2::Diff::new(vec![change.file_type(v2::DiffFileType::Text)])
        .with_patch(patch.map(v2::DiffPatch::new))
        .meta(meta)
}

/// v2 tags free-text command input as `type: "text"`.
pub(super) fn available_commands(
    commands: Vec<v1::AvailableCommand>,
) -> Result<Vec<v2::AvailableCommand>, AcpError> {
    commands
        .into_iter()
        .map(|command| {
            let input = match command.input {
                Some(v1::AvailableCommandInput::Unstructured(input)) => {
                    Some(v2::AvailableCommandInput::Text(
                        v2::TextCommandInput::new(input.hint).meta(input.meta),
                    ))
                }
                None => None,
                Some(_) => {
                    return Err(AcpError::internal_error().data("no ACP v2 form for command input"))
                }
            };
            Ok(v2::AvailableCommand::new(command.name, command.description)
                .input(input)
                .meta(command.meta))
        })
        .collect()
}

/// v2 renames a config option's `id` to `configId` and a select group's
/// `group` to `groupId`.
pub(super) fn config_options(
    options: Vec<v1::SessionConfigOption>,
) -> Result<Vec<v2::SessionConfigOption>, AcpError> {
    options.into_iter().map(config_option).collect()
}

fn config_option(option: v1::SessionConfigOption) -> Result<v2::SessionConfigOption, AcpError> {
    let kind = match option.kind {
        v1::SessionConfigKind::Select(select) => {
            let options = match select.options {
                v1::SessionConfigSelectOptions::Ungrouped(options) => {
                    v2::SessionConfigSelectOptions::Ungrouped(bridge(&options)?)
                }
                v1::SessionConfigSelectOptions::Grouped(groups) => {
                    v2::SessionConfigSelectOptions::Grouped(
                        groups
                            .into_iter()
                            .map(|group| {
                                Ok(v2::SessionConfigSelectGroup::new(
                                    group.group.0.clone(),
                                    group.name,
                                    bridge(&group.options)?,
                                )
                                .meta(group.meta))
                            })
                            .collect::<Result<_, AcpError>>()?,
                    )
                }
                _ => {
                    return Err(AcpError::internal_error().data("no ACP v2 form for these options"))
                }
            };
            v2::SessionConfigKind::Select(v2::SessionConfigSelect::new(
                select.current_value.0.clone(),
                options,
            ))
        }
        v1::SessionConfigKind::Boolean(boolean) => {
            v2::SessionConfigKind::Boolean(v2::SessionConfigBoolean::new(boolean.current_value))
        }
        _ => return Err(AcpError::internal_error().data("no ACP v2 form for this config option")),
    };
    Ok(
        v2::SessionConfigOption::new(option.id.0.clone(), option.name, kind)
            .description(option.description)
            .category(
                option
                    .category
                    .map(|category| bridge(&category))
                    .transpose()?,
            )
            .meta(option.meta),
    )
}

#[cfg(test)]
#[path = "updates_tests.rs"]
mod tests;
