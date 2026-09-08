use crate::{model::Message, tool::ToolOutput};

/// Keep tool-owned content explicitly separate from actual user instructions.
pub(super) fn supplemental_output(name: &str, id: &str, output: &ToolOutput) -> Option<Message> {
    Message::tool_image_supplement(name, id, output.images().to_vec())
}
