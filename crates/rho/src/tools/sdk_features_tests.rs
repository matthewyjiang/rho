use std::{
    num::NonZeroUsize,
    sync::{Arc, Mutex},
};

use pretty_assertions::assert_eq;
use rho_sdk::{
    tool::{tool_progress_channel, ToolContext, ToolErrorKind, ToolInvocation},
    CancellationToken, ToolCallId,
};
use serde_json::json;

use super::message_parent_bundle;
use crate::{
    app::subagent_messaging::{NoticeDelivery, NoticePostError, NoticePoster, ValidatedMessage},
    tools::sdk_registry::ToolBundle,
};

#[derive(Default)]
struct RecordingPoster(Mutex<Vec<(ValidatedMessage, NoticeDelivery)>>);

impl NoticePoster for RecordingPoster {
    fn post(
        &self,
        message: ValidatedMessage,
        delivery: NoticeDelivery,
    ) -> Result<(), NoticePostError> {
        self.0.lock().unwrap().push((message, delivery));
        Ok(())
    }
}

// Covers: ordinary messages must not request a wake; action requests must not silently wait.
// Owner: child communication tool dispatch and argument validation.
#[tokio::test]
async fn subagent_notice_tools_route_delivery_and_reject_empty_messages() {
    let poster = Arc::new(RecordingPoster::default());
    let bundle = message_parent_bundle(poster.clone());
    for (name, delivery) in [
        ("message_parent", NoticeDelivery::NextTurn),
        (
            "request_parent_action",
            NoticeDelivery::ParentActionRequired,
        ),
    ] {
        let tool = bundle
            .tools()
            .iter()
            .find(|tool| tool.spec().name == name)
            .unwrap();
        for message in ["  coordinate file ownership  ", " \n "] {
            let (progress, _receiver) = tool_progress_channel(NonZeroUsize::MIN);
            let context = ToolContext::new(None, CancellationToken::new(), progress);
            let result = tool
                .call(
                    ToolInvocation::new(ToolCallId::new(), json!({"message": message})),
                    context,
                )
                .await;
            if message.trim().is_empty() {
                assert_eq!(result.unwrap_err().kind(), ToolErrorKind::InvalidArguments);
                assert_eq!(*poster.0.lock().unwrap(), vec![]);
            } else {
                result.unwrap();
                assert_eq!(
                    std::mem::take(&mut *poster.0.lock().unwrap()),
                    vec![(ValidatedMessage::parse(message).unwrap(), delivery)]
                );
            }
        }
    }
}
