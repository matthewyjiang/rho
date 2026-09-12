use super::*;
use pretty_assertions::assert_eq;

// Covers: patch path syntax must not bypass or preempt authorization, and a denied
// target must prevent every operation in a multi-file patch from mutating files.
// Owner: SDK tool execution contract
#[tokio::test]
async fn patch_paths_follow_workspace_authorization() {
    for path_style in ["absolute", "parent", "symlink"] {
        if path_style == "symlink" && !cfg!(unix) {
            continue;
        }
        for permission in [
            "allow",
            "deny outside",
            "deny read",
            "deny write",
            "restricted",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().join("workspace");
            let outside = dir.path().join("outside");
            std::fs::create_dir_all(&root).unwrap();
            std::fs::create_dir_all(&outside).unwrap();
            std::fs::write(root.join("local.txt"), "old\n").unwrap();
            std::fs::write(outside.join("source.txt"), "source\n").unwrap();
            std::fs::write(outside.join("delete.txt"), "delete\n").unwrap();
            #[cfg(unix)]
            std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
            let prefix = match path_style {
                "absolute" => outside.display().to_string(),
                "parent" => "../outside".into(),
                "symlink" => "link".into(),
                _ => unreachable!(),
            };
            let input = format!(
                "*** Begin Patch\n*** Update File: local.txt\n@@\n-old\n+updated\n*** Add File: {prefix}/added.txt\n+added\n*** Update File: {prefix}/source.txt\n*** Move to: {prefix}/moved.txt\n@@\n-source\n+moved\n*** Delete File: {prefix}/delete.txt\n*** End Patch"
            );
            let mut workspace = Workspace::new(&root).unwrap();
            if permission != "restricted" {
                workspace = workspace.with_unrestricted_file_access();
            }
            let mut policy = ScopedWorkspacePolicy::new();
            if permission != "deny read" {
                policy = policy.allow_read_paths();
            }
            if permission != "deny write" {
                policy = policy.allow_write_paths();
            }
            if permission != "deny outside" {
                policy = policy.allow_outside_workspace_paths();
            }
            let runtime = build_runtime_with_coding_tools(
                ScriptedProvider::new(
                    ModelIdentity::new("scripted", "test", "model"),
                    [
                        ScriptedTurn::completed(ModelResponse::Assistant(vec![
                            ContentBlock::ToolCall(ToolCall {
                                id: "call-1".into(),
                                name: "apply_patch".into(),
                                arguments: json!({"input": input}),
                            }),
                        ])),
                        ScriptedTurn::completed(ModelResponse::Assistant(vec![
                            ContentBlock::Text("done".into()),
                        ])),
                    ],
                ),
                workspace,
                policy,
                CodingToolOptions::new().edit_tool(crate::EditFormat::ApplyPatch),
            );
            let session = runtime.session(SessionOptions::default()).await.unwrap();
            let mut run = session
                .start(UserInput::text("apply the patch"))
                .await
                .unwrap();
            let mut completion = None;
            while let Some(event) = run.next_event().await {
                if let RunEvent::ToolFinished { result, .. } = event {
                    completion = Some(result);
                }
            }
            run.outcome().await.unwrap();
            let allowed = permission == "allow";
            match completion.expect("patch completion") {
                ToolCompletion::Success(_) if allowed => {}
                ToolCompletion::Failure(failure) if !allowed => {
                    assert_eq!(
                        failure.kind(),
                        ToolErrorKind::PolicyDenied,
                        "{path_style}: {permission}"
                    );
                }
                other => panic!("{path_style}: {permission}: unexpected {other:?}"),
            }
            let snapshot = [
                root.join("local.txt"),
                outside.join("added.txt"),
                outside.join("source.txt"),
                outside.join("moved.txt"),
                outside.join("delete.txt"),
            ]
            .map(|path| match std::fs::read_to_string(path) {
                Ok(content) => Some(content),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => panic!("could not read patch result: {error}"),
            });
            let expected = if allowed {
                [
                    Some("updated\n"),
                    Some("added\n"),
                    None,
                    Some("moved\n"),
                    None,
                ]
            } else {
                [
                    Some("old\n"),
                    None,
                    Some("source\n"),
                    None,
                    Some("delete\n"),
                ]
            }
            .map(|content| content.map(str::to_owned));
            assert_eq!(snapshot, expected, "{path_style}: {permission}");
        }
    }
}
