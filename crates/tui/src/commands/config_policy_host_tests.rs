//! Public registry and optional-observation regressions for the policy slice.
use crate::commands::traits::CommandGroup;
use codewhale_command_contract::handler::{CommandCapabilities as Caps, CommandHandler};

#[test]
fn config_policy_keeps_host_registry_order_metadata_and_exact_capabilities() {
    let group = crate::commands::groups::config::ConfigCommands;
    assert_eq!(
        group
            .commands()
            .iter()
            .map(|command| command.info().name)
            .collect::<Vec<_>>(),
        [
            "config",
            "import-claude",
            "permissions",
            "login",
            "auth",
            "workbar",
            "pet",
            "settings",
            "status",
            "statusline",
            "mode",
            "fullscreen",
            "inline",
            "theme",
            "verbose",
            "trust",
            "logout"
        ]
    );
    for (info, handler) in crate::commands::groups::config::policy::portable_handlers() {
        let CommandHandler::Contextual {
            capabilities: expected,
            ..
        } = handler
        else {
            panic!("portable contextual handler")
        };
        assert_eq!(
            expected,
            if info.name == "permissions" {
                Caps::PERMISSIONS | Caps::PRESENTATION
            } else {
                Caps::CONFIG_STATUS | Caps::PRESENTATION
            }
        );
        for name in std::iter::once(info.name).chain(info.aliases.iter().copied()) {
            let registered = crate::commands::registry().get(name).unwrap();
            assert_eq!(registered.info().name, info.name);
            assert_eq!(registered.info().aliases, info.aliases);
            assert_eq!(registered.info().usage, info.usage);
            assert_eq!(
                Some(registered.info().description_id),
                crate::commands::contract::key_to_message_id(info.description_key)
            );
            let CommandHandler::Contextual { capabilities, .. } =
                registered.contextual_handler().unwrap()
            else {
                panic!("host contextual handler")
            };
            assert_eq!(capabilities, expected);
        }
    }
}

#[test]
fn config_policy_status_survives_unreadable_optional_config_without_rewriting_it() {
    let _env = crate::test_support::lock_test_env();
    let temp = tempfile::TempDir::new().unwrap();
    let mut app = crate::test_support::test_app_with_options(
        crate::test_support::test_tui_options(temp.path()),
    );
    app.ui_locale = codewhale_localization::Locale::En;
    let path = temp.path().join("config.toml");
    let original = "provider = [malformed optional config with {braces}\n";
    std::fs::write(&path, original).unwrap();
    app.config_path = Some(path.clone());
    app.model = "retain-this-pin".into();
    app.current_session_id = Some("retain-this-session".into());
    for _ in 0..2 {
        let result = crate::commands::execute("/status", &mut app);
        assert!(!result.is_error, "{result:?}");
        assert!(result.action.is_none());
        let text = result.message.unwrap();
        assert!(text.contains("retain-this-pin"));
        assert!(text.contains("retain-this-session"));
        assert!(!text.contains("malformed optional config"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    }
    assert_eq!(app.model, "retain-this-pin");
    assert_eq!(
        app.current_session_id.as_deref(),
        Some("retain-this-session")
    );
}
