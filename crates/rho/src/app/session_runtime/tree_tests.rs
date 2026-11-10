use pretty_assertions::assert_eq;
use rho_sdk::model::{ContentBlock, Message};

use super::SelectNodeError;
use crate::{
    app::session_runtime::{test_support::idle_runtime, SessionRuntime},
    config::Config,
    session::tree::NodeId,
};

/// The text of each user message the model would read.
fn user_texts(runtime: &SessionRuntime) -> Vec<String> {
    runtime
        .built
        .session
        .history()
        .into_iter()
        .filter_map(|message| match message {
            Message::User(blocks) => Some(
                blocks
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::Text(text) => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<String>(),
            ),
            _ => None,
        })
        .collect()
}

fn leaf(runtime: &SessionRuntime) -> Option<NodeId> {
    let stored = runtime.storage.file().expect("the session is saved");
    stored
        .active_checkpoint_target()
        .unwrap()
        .map(|(node, _)| node)
}

// Covers: selecting a node moves what the model reads, not only the file's
// leaf, back to that node, so the next turn continues from it; a node the
// tree does not hold, or a leaf the file could not record, changes neither.
// Owner: session runtime tree selection
#[tokio::test]
async fn selecting_a_node_moves_the_model_conversation_and_the_leaf_to_it() {
    let home = tempfile::tempdir().unwrap();
    let config = Config {
        provider: "test".into(),
        model: "model".into(),
        ..Config::default()
    };
    let mut runtime = idle_runtime(home.path(), &config).await;
    let add = |runtime: &mut SessionRuntime, text: &str| {
        runtime
            .append_context(text.into(), Message::user_text(text), [])
            .unwrap();
        leaf(runtime).expect("the row saved a node")
    };
    let first = add(&mut runtime, "first");
    let second = add(&mut runtime, "second");

    let unknown = NodeId::from_string("no-such-node").unwrap();
    let stored = runtime.storage.file().unwrap().clone();
    stored.fail_next_leaf_commit_for_tests();
    let unrecorded = runtime.select_node(&first, &config).await.is_err();
    let after_failed_write = (user_texts(&runtime), leaf(&runtime));
    let refused = matches!(
        runtime.select_node(&unknown, &config).await,
        Err(SelectNodeError::UnknownNode(_))
    );
    let after_refusal = (user_texts(&runtime), leaf(&runtime));
    let selected = runtime
        .select_node(&first, &config)
        .await
        .map_err(|error| error.to_string());

    let both = (vec!["first".to_owned(), "second".to_owned()], Some(second));
    assert_eq!(
        (
            (refused, unrecorded),
            after_refusal,
            after_failed_write,
            selected,
            user_texts(&runtime),
            leaf(&runtime)
        ),
        (
            (true, true),
            both.clone(),
            both,
            Ok(()),
            vec!["first".to_owned()],
            Some(first),
        )
    );
    runtime.shutdown().await;
}
