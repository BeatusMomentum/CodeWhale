//! Exercise command dispatch, the actual UI apply path, Engine requests and disk.
use super::*;
use crate::core::engine::{Engine, EngineConfig};
use crate::core::events::TurnOutcomeStatus;
use crate::llm_client::mock::{MockLlmClient, canned};
use codewhale_models::{ContentBlock, Message};

fn prompts(messages: &[Message]) -> Vec<String> {
    messages
        .iter()
        .filter(|message| {
            matches!(
                crate::runtime_handoff::classify_user_turn_prompt(message),
                crate::runtime_handoff::UserTurnPromptKind::Editable
            )
        })
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            ContentBlock::Text { text, .. } if !text.starts_with("<turn_meta>") => {
                Some(text.clone())
            }
            _ => None,
        })
        .collect()
}

async fn settled(app: &mut App, handle: &EngineHandle, answer: &str) {
    let mut events = handle.rx_event.write().await;
    loop {
        let event = tokio::time::timeout(Duration::from_secs(30), events.recv())
            .await
            .expect("turn deadline")
            .expect("Engine event");
        if let EngineEvent::TurnComplete { status, error, .. } = event {
            assert_eq!(status, TurnOutcomeStatus::Completed, "{error:?}");
            break;
        }
    }
    drop(events);
    let snapshot = handle
        .get_session_snapshot()
        .await
        .expect("settled snapshot");
    app.current_session_id = Some(snapshot.session_id);
    app.set_api_messages(Arc::new(snapshot.messages));
    app.system_prompt = snapshot.system_prompt;
    app.model = snapshot.model;
    app.is_loading = false;
    app.push_history_cell(HistoryCell::Assistant {
        content: answer.into(),
        streaming: false,
    });
}

#[test]
fn undo_retry_apply_path_keeps_engine_request_and_reopened_session_consistent() {
    const PROBE: &str = "CODEWHALE_UNDO_RETRY_APPLY_PROBE";
    if std::env::var_os(PROBE).is_none() {
        // The production actor is process-global. Never replace another
        // test's actor or leave this fixture's closed sender in its process.
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tui::ui::tests::conversation_undo::undo_retry_apply_path_keeps_engine_request_and_reopened_session_consistent",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(PROBE, "1")
            .output()
            .expect("isolated UI lifecycle");
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        return;
    }
    let _environment = crate::test_support::lock_test_env();
    let home = TempDir::new().unwrap();
    let workspace = TempDir::new().unwrap();
    let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", home.path());
    let _user_home = crate::test_support::EnvVarGuard::set("HOME", home.path());
    let _profile = crate::test_support::EnvVarGuard::set("USERPROFILE", home.path());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let mut config = Config::default().with_legacy_root(
            Some("mock-credential".into()),
            Some("http://127.0.0.1:1/v1".into()),
        );
        config.set_feature("mcp", false).unwrap();
        config.set_feature("subagents", false).unwrap();
        let mut app = App::new(
            crate::test_support::test_tui_options(workspace.path()),
            &config,
        );
        app.onboarding_needs_api_key = false;
        app.offline_mode = false;
        app.mode = AppMode::Agent;
        let mock = Arc::new(MockLlmClient::new(vec![
            canned::simple_text_turn("kept answer"),
            canned::simple_text_turn("undone answer"),
            canned::simple_text_turn("retry answer one"),
            canned::simple_text_turn("retry answer two"),
            canned::simple_text_turn("answer after reopen"),
        ]));
        let (engine, mut handle) = Engine::new_with_model_client(
            EngineConfig {
                workspace: workspace.path().to_path_buf(),
                model: app.model.clone(),
                snapshots_enabled: false,
                subagents_enabled: false,
                terminal_chrome_enabled: false,
                ..EngineConfig::default()
            },
            &config,
            mock.clone(),
        );
        let engine_task = tokio::spawn(engine.run());
        let manager = SessionManager::default_location().unwrap();
        let (actor, actor_task) =
            persistence_actor::spawn_persistence_actor(SessionManager::default_location().unwrap());
        persistence_actor::init_actor(actor.clone());
        let tasks = TaskManager::start(
            TaskManagerConfig::from_runtime(&config, workspace.path().into(), None, Some(1)),
            config.clone(),
            app.plugin_registry.clone(),
            "undo-fixture-tasks",
            None,
        )
        .await
        .unwrap();
        let mut backend = ColorCompatBackend::new(
            std::io::stdout(),
            codewhale_palette::ColorDepth::Monochrome,
            codewhale_palette::PaletteMode::Dark,
        );
        backend.set_terminal_size(Size::new(80, 24));
        let mut terminal = Terminal::new(backend).unwrap();

        for (prompt, answer) in [
            ("keep this", "kept answer"),
            ("retry this", "undone answer"),
        ] {
            apply_command_result(
                &mut terminal,
                &mut app,
                &mut handle,
                &tasks,
                &mut config,
                commands::CommandResult {
                    message: None,
                    action: Some(AppAction::SendMessage(prompt.into())),
                    is_error: false,
                },
            )
            .await
            .unwrap();
            settled(&mut app, &handle, answer).await;
        }
        let id = app.current_session_id.clone().unwrap();
        let old = build_session_snapshot(&mut app, &manager).unwrap();
        let path = manager.save_session(&old).unwrap();
        // The stale in-flight recovery record must not revive the undone turn.
        assert!(actor.try_send(PersistRequest::SaveCheckpoint { session: old }));

        for answer in ["retry answer one", "retry answer two"] {
            let result = commands::execute("/retry", &mut app);
            assert!(matches!(
                result.action,
                Some(AppAction::ConversationUndo { .. })
            ));
            apply_command_result(
                &mut terminal,
                &mut app,
                &mut handle,
                &tasks,
                &mut config,
                result,
            )
            .await
            .unwrap();
            settled(&mut app, &handle, answer).await;
            let requests = mock.captured_requests();
            let last = requests.last().unwrap();
            assert_eq!(prompts(&last.messages), ["keep this", "retry this"]);
            assert!(
                !serde_json::to_string(&last.messages)
                    .unwrap()
                    .contains("undone answer")
            );
            let reopened = manager.load_session(&id).unwrap();
            assert_eq!(
                prompts(&reopened.messages),
                ["keep this"],
                "rollback is durable before replacement inference"
            );
            assert!(manager.load_session_checkpoint(&id).unwrap().is_none());
        }
        assert_eq!(mock.captured_requests().len(), 4);

        // Runtime Chat ownership and a locally active turn must refuse before
        // even staging a different Engine history or altering the transcript.
        let before = app.api_messages.clone();
        app.remote_control.block_runtime_chat_dispatch_for_tests();
        let result = commands::execute("/retry", &mut app);
        apply_command_result(
            &mut terminal,
            &mut app,
            &mut handle,
            &tasks,
            &mut config,
            result,
        )
        .await
        .unwrap();
        assert_eq!(app.api_messages, before);
        assert_eq!(mock.captured_requests().len(), 4);
        app.remote_control = Default::default();
        app.is_loading = true;
        let result = commands::execute("/retry", &mut app);
        apply_command_result(
            &mut terminal,
            &mut app,
            &mut handle,
            &tasks,
            &mut config,
            result,
        )
        .await
        .unwrap();
        assert_eq!(app.api_messages, before);
        app.is_loading = false;

        let result = commands::execute("/undo", &mut app);
        apply_command_result(
            &mut terminal,
            &mut app,
            &mut handle,
            &tasks,
            &mut config,
            result,
        )
        .await
        .unwrap();
        let engine_saved = handle.get_session_snapshot().await.unwrap();
        let reopened = SessionManager::default_location()
            .unwrap()
            .load_session(&id)
            .unwrap();
        assert_eq!(prompts(&engine_saved.messages), ["keep this"]);
        assert_eq!(prompts(&app.api_messages), ["keep this"]);
        assert_eq!(prompts(&reopened.messages), ["keep this"]);
        assert!(
            !serde_json::to_string(&reopened.messages)
                .unwrap()
                .contains("retry answer")
        );
        assert_eq!(
            mock.captured_requests().len(),
            4,
            "undo performs no inference"
        );

        // Resume the durable record through the existing UI projection and
        // SyncSession apply action, then inspect the actual next model request.
        // File LoadSession always respawns a real client, so use the same
        // loaded-session projection with our injected model client here.
        apply_loaded_session(&mut app, &mut config, &reopened).unwrap();
        let restore = AppAction::SyncSession {
            session_id: app.current_session_id.clone(),
            messages: app.api_messages.as_ref().clone(),
            system_prompt: app.system_prompt.clone(),
            model: app.model.clone(),
            workspace: app.workspace.clone(),
            mode: app.mode,
        };
        for action in [restore, AppAction::SendMessage("after reopen".into())] {
            apply_command_result(
                &mut terminal,
                &mut app,
                &mut handle,
                &tasks,
                &mut config,
                commands::CommandResult {
                    message: None,
                    action: Some(action),
                    is_error: false,
                },
            )
            .await
            .unwrap();
        }
        settled(&mut app, &handle, "answer after reopen").await;
        let requests = mock.captured_requests();
        assert_eq!(requests.len(), 5);
        assert_eq!(
            prompts(&requests.last().unwrap().messages),
            ["keep this", "after reopen"]
        );
        let inbound = serde_json::to_string(&requests.last().unwrap().messages).unwrap();
        assert!(!inbound.contains("retry this"));
        assert!(!inbound.contains("undone answer"));
        assert!(!inbound.contains("retry answer"));

        // Return to the retained exchange before probing durable-save failure.
        let result = commands::execute("/undo", &mut app);
        apply_command_result(
            &mut terminal,
            &mut app,
            &mut handle,
            &tasks,
            &mut config,
            result,
        )
        .await
        .unwrap();
        assert_eq!(prompts(&app.api_messages), ["keep this"]);

        // Portable save failure: block the canonical file with a directory.
        // Preserve the last durable snapshot as a sibling for inspection.
        std::fs::rename(&path, path.with_extension("before-failure")).unwrap();
        std::fs::create_dir(&path).unwrap();
        let result = commands::execute("/retry", &mut app);
        apply_command_result(
            &mut terminal,
            &mut app,
            &mut handle,
            &tasks,
            &mut config,
            result,
        )
        .await
        .unwrap();
        assert_eq!(
            mock.captured_requests().len(),
            5,
            "save failure must forbid retry inference"
        );
        assert!(
            app.status_toasts
                .back()
                .unwrap()
                .text
                .contains("session save failed")
        );
        assert!(
            app.api_messages.is_empty(),
            "acknowledged undo remains visible after save failure"
        );
        assert!(
            handle
                .get_session_snapshot()
                .await
                .unwrap()
                .messages
                .is_empty()
        );

        handle.send(Op::Shutdown).await.unwrap();
        engine_task.await.unwrap();
        app.set_api_messages(Arc::new(reopened.messages));
        app.push_history_cell(HistoryCell::User {
            content: "keep this".into(),
        });
        let result = commands::execute("/retry", &mut app);
        let before = app.api_messages.clone();
        apply_command_result(
            &mut terminal,
            &mut app,
            &mut handle,
            &tasks,
            &mut config,
            result,
        )
        .await
        .unwrap();
        assert_eq!(
            app.api_messages, before,
            "closed Engine must leave the visible conversation alone"
        );
        assert!(
            app.status_toasts
                .back()
                .unwrap()
                .text
                .contains("retry was not sent")
        );
        assert_eq!(mock.captured_requests().len(), 5);
        tasks.shutdown_and_wait().await.unwrap();
        assert!(actor.try_send(PersistRequest::Shutdown));
        actor_task.await.unwrap();
    });
}
