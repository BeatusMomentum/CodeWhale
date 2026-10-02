//! Real programmable host listeners on the actual Engine admission path.
use super::*;
use crate::llm_client::mock::{MockLlmClient, canned};

#[tokio::test]
async fn typescript_mod_rewrites_denies_and_regates_native_tools_on_the_real_engine() {
    let Some(node) = crate::extension_host::tests::node_for_tests("typescript_mod_real_engine")
    else {
        return;
    };
    let _policy = crate::plugins::activation::TestPolicyGuard::extension_host(true);
    let fixture = crate::extension_host::tests::FixturePlugins::new(&["hook-policy"]).await;
    for (name, text) in [
        ("before.txt", "original"),
        ("after.txt", "rewritten content"),
        ("blocked.txt", "must not be returned"),
        ("malformed.txt", "must not be returned"),
        ("throw.txt", "must not be returned"),
        ("rewrite-action.txt", "original read"),
    ] {
        fs::write(fixture.workspace().join(name), text).unwrap();
    }
    let manager = fixture.manager(node);
    let warm = manager.attach(fixture.registry());
    warm.sync().await.expect("real host activation");
    let _manager = crate::extension_host::TestManagerGuard::install(Arc::clone(&manager));
    let mock = Arc::new(MockLlmClient::new(vec![
        canned::tool_call_turn("rewrite", "read", r#"{"path":"before.txt"}"#),
        canned::tool_call_turn("deny", "read", r#"{"path":"blocked.txt"}"#),
        canned::tool_call_turn("malformed", "read", r#"{"path":"malformed.txt"}"#),
        canned::tool_call_turn("throw", "read", r#"{"path":"throw.txt"}"#),
        canned::tool_call_turn(
            "action",
            "File",
            r#"{"action":"read","path":"rewrite-action.txt"}"#,
        ),
        canned::simple_text_turn("Finished checking the mod."),
        canned::tool_call_turn("after-disable", "read", r#"{"path":"before.txt"}"#),
        canned::simple_text_turn("The mod is disabled."),
    ]));
    let config = Config::default();
    let mut engine_config = deterministic_engine_config(fixture.workspace());
    engine_config.features.enable(Feature::ExtensionHost);
    engine_config.plugin_registry = Some(fixture.registry());
    let (engine, handle) = Engine::new_with_model_client(engine_config, &config, mock.clone());
    manager.reconcile().await.unwrap();
    let task = tokio::spawn(engine.run());
    handle
        .send(external_user_message_op(
            "Check the programmable mod",
            AppMode::Agent,
            &config,
        ))
        .await
        .unwrap();
    let mut outcomes = HashMap::new();
    let mut approvals = 0;
    let mut rx = handle.rx_event.write().await;
    loop {
        match tokio::time::timeout(model_turn_event_timeout(), rx.recv())
            .await
            .unwrap()
            .unwrap()
        {
            Event::ApprovalRequired {
                id, description, ..
            } => {
                approvals += 1;
                assert!(
                    description.contains("rewritten.txt"),
                    "the revised write must be prepared again: {description}"
                );
                handle.deny_tool_call(id).await.unwrap();
            }
            Event::ToolCallComplete {
                model_call: Some(call),
                result,
                ..
            } => {
                outcomes.insert(call.provider_id, result);
            }
            Event::TurnComplete { status, error, .. } => {
                assert_eq!(status, TurnOutcomeStatus::Completed, "{error:?}");
                break;
            }
            _ => {}
        }
    }
    assert!(
        outcomes
            .remove("rewrite")
            .unwrap()
            .unwrap()
            .content
            .contains("rewritten content")
    );
    assert!(
        outcomes
            .remove("deny")
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("blocked by fixture")
    );
    for id in ["malformed", "throw"] {
        assert!(
            outcomes
                .remove(id)
                .unwrap()
                .unwrap_err()
                .to_string()
                .contains("returned no verdict")
        );
    }
    assert!(outcomes.remove("action").unwrap().is_err());
    assert_eq!(approvals, 1, "only the revised write needs an approval");
    assert!(!fixture.workspace().join("rewritten.txt").exists());
    drop(rx);
    let disabled = fixture.disable("hook-policy");
    crate::extension_host::plugins_changed(disabled);
    manager.reconcile().await.unwrap();
    handle
        .send(external_user_message_op(
            "Read again after disabling",
            AppMode::Agent,
            &config,
        ))
        .await
        .unwrap();
    let mut rx = handle.rx_event.write().await;
    let mut original = false;
    loop {
        match tokio::time::timeout(model_turn_event_timeout(), rx.recv())
            .await
            .unwrap()
            .unwrap()
        {
            Event::ToolCallComplete {
                model_call: Some(call),
                result: Ok(result),
                ..
            } if call.provider_id == "after-disable" => {
                original = result.content.contains("original")
                    && !result.content.contains("rewritten content");
            }
            Event::ApprovalRequired { .. } => panic!("disabled hook cannot request approval"),
            Event::TurnComplete { status, error, .. } => {
                assert_eq!(status, TurnOutcomeStatus::Completed, "{error:?}");
                break;
            }
            _ => {}
        }
    }
    assert!(original, "disabled listener must not rewrite the next turn");
    drop(rx);
    handle.send(Op::Shutdown).await.unwrap();
    task.await.unwrap();
    manager.shutdown().await;
}
