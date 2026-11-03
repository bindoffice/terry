// Dispatcher-level witnesses for the pi-parity RPC commands.
//
// Each test drives the real `rpc_with_io` loop, so it exercises framing, the
// command whitelist and the response envelope together — the three places a new
// command is most likely to be forgotten.

mod parity_commands {
    use super::*;
    use std::time::Duration;

    const LIMIT: Duration = Duration::from_secs(10);

    /// Run `frames` through the dispatcher and return its wire output.
    async fn dispatch(frames: &str) -> Vec<serde_json::Value> {
        let (harness, _) = rpc_test_harness("unused", 0.0).await;
        let output = RpcOutput::default();
        let code = tokio::time::timeout(
            LIMIT,
            rpc_with_io(
                &harness,
                None,
                vec![],
                tom_extensions::UiDialogMailbox::new(),
                None,
                crate::session::SessionEventGate::none(),
                crate::args::RpcMessageUpdates::default(),
                tokio::io::BufReader::new(frames.as_bytes()),
                output.clone(),
                crate::args::Args::default(),
                std::path::PathBuf::from("."),
            ),
        )
        .await
        .expect("dispatcher must finish");
        assert_eq!(code, 0, "dispatcher exited with {code}");
        output.messages()
    }

    fn replies(messages: &[serde_json::Value]) -> Vec<&serde_json::Value> {
        messages
            .iter()
            .filter(|message| message["type"] == "response")
            .collect()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ready_advertises_every_pi_command() {
        let messages = dispatch("{\"id\":1,\"type\":\"ping\"}\n").await;
        let ready = messages
            .iter()
            .find(|message| message["type"] == "ready")
            .expect("ready frame");
        let advertised: Vec<&str> = ready["supportedCommands"]
            .as_array()
            .expect("supportedCommands array")
            .iter()
            .filter_map(|value| value.as_str())
            .collect();

        // The pi reference command list. If a command is dropped from the
        // whitelist this test names it instead of silently losing parity.
        for command in [
            "abort",
            "abort_bash",
            "abort_retry",
            "bash",
            "clear_queue",
            "clone",
            "compact",
            "cycle_model",
            "cycle_thinking_level",
            "export_html",
            "follow_up",
            "fork",
            "get_available_models",
            "get_available_thinking_levels",
            "get_commands",
            "get_entries",
            "get_fork_messages",
            "get_last_assistant_text",
            "get_messages",
            "get_session_stats",
            "get_state",
            "get_tree",
            "new_session",
            "prompt",
            "set_auto_compaction",
            "set_auto_retry",
            "set_follow_up_mode",
            "set_model",
            "set_session_name",
            "set_steering_mode",
            "set_thinking_level",
            "steer",
            "switch_session",
        ] {
            assert!(
                advertised.contains(&command),
                "`{command}` is not advertised; ready said: {advertised:?}"
            );
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn auto_compaction_and_retry_toggle_and_report() {
        let messages = dispatch(concat!(
            "{\"id\":1,\"type\":\"set_auto_compaction\",\"enabled\":false}\n",
            "{\"id\":2,\"type\":\"set_auto_retry\",\"enabled\":false}\n",
            "{\"id\":3,\"type\":\"set_auto_compaction\",\"enabled\":true}\n",
        ))
        .await;
        let replies = replies(&messages);
        assert_eq!(replies.len(), 3, "{messages:?}");
        assert_eq!(replies[0]["data"]["enabled"], false);
        assert_eq!(replies[1]["data"]["enabled"], false);
        assert_eq!(replies[2]["data"]["enabled"], true);
        for reply in replies {
            assert_eq!(reply["success"], true, "{reply:?}");
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn queue_modes_accept_all_and_one_at_a_time() {
        let messages = dispatch(concat!(
            "{\"id\":1,\"type\":\"set_steering_mode\",\"mode\":\"all\"}\n",
            "{\"id\":2,\"type\":\"set_follow_up_mode\",\"mode\":\"one-at-a-time\"}\n",
        ))
        .await;
        let replies = replies(&messages);
        assert_eq!(replies.len(), 2, "{messages:?}");
        assert_eq!(replies[0]["data"]["mode"], "all");
        assert_eq!(replies[1]["data"]["mode"], "one-at-a-time");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn queue_mode_rejects_an_unknown_value() {
        let messages =
            dispatch("{\"id\":1,\"type\":\"set_steering_mode\",\"mode\":\"sometimes\"}\n").await;
        let reply = replies(&messages)[0];
        assert_eq!(reply["success"], false, "{reply:?}");
        let error = reply["error"].as_str().unwrap_or_default();
        assert!(error.contains("unknown mode"), "{error}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn steer_and_follow_up_queue_a_message() {
        let messages = dispatch(concat!(
            "{\"id\":1,\"type\":\"steer\",\"message\":\"go left\"}\n",
            "{\"id\":2,\"type\":\"follow_up\",\"message\":\"then summarise\"}\n",
            "{\"id\":3,\"type\":\"get_queue\"}\n",
        ))
        .await;
        let replies = replies(&messages);
        assert_eq!(replies[0]["data"]["disposition"], "queued", "{messages:?}");
        assert_eq!(replies[1]["data"]["disposition"], "queued", "{messages:?}");
        let queue = replies[2]["data"]["items"].as_array().expect("queue items");
        assert_eq!(queue.len(), 2, "{queue:?}");
        let texts: Vec<&str> = queue
            .iter()
            .filter_map(|item| item["text"].as_str())
            .collect();
        assert!(texts.contains(&"go left"), "{texts:?}");
        assert!(texts.contains(&"then summarise"), "{texts:?}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn steer_requires_a_message() {
        let messages = dispatch("{\"id\":1,\"type\":\"steer\",\"message\":\"  \"}\n").await;
        let reply = replies(&messages)[0];
        assert_eq!(reply["success"], false, "{reply:?}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn set_session_name_sets_and_clears() {
        let messages = dispatch(concat!(
            "{\"id\":1,\"type\":\"set_session_name\",\"name\":\"my session\"}\n",
            "{\"id\":2,\"type\":\"get_state\"}\n",
            "{\"id\":3,\"type\":\"set_session_name\",\"name\":null}\n",
        ))
        .await;
        let replies = replies(&messages);
        assert_eq!(replies[0]["data"]["name"], "my session", "{messages:?}");
        assert_eq!(replies[2]["data"]["name"], serde_json::Value::Null);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn session_inspection_commands_answer_on_an_empty_session() {
        let messages = dispatch(concat!(
            "{\"id\":1,\"type\":\"get_tree\"}\n",
            "{\"id\":2,\"type\":\"get_entries\"}\n",
            "{\"id\":3,\"type\":\"get_fork_messages\"}\n",
            "{\"id\":4,\"type\":\"get_last_assistant_text\"}\n",
            "{\"id\":5,\"type\":\"get_session_stats\"}\n",
        ))
        .await;
        let replies = replies(&messages);
        assert_eq!(replies.len(), 5, "{messages:?}");
        for reply in &replies {
            assert_eq!(reply["success"], true, "{reply:?}");
        }
        assert_eq!(replies[0]["data"]["count"], 0);
        assert_eq!(replies[1]["data"]["count"], 0);
        assert_eq!(replies[2]["data"]["count"], 0);
        // No assistant turn yet, so the text is null rather than absent.
        assert_eq!(replies[3]["data"]["text"], serde_json::Value::Null);
        assert!(replies[4]["data"].get("context").is_some(), "{replies:?}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cycle_thinking_level_reports_the_new_level() {
        let messages = dispatch("{\"id\":1,\"type\":\"cycle_thinking_level\"}\n").await;
        let reply = replies(&messages)[0];
        assert_eq!(reply["success"], true, "{reply:?}");
        // Either a level name or null (a model without thinking support); both
        // are valid per the reference, so the assertion is on the shape.
        assert!(
            reply["data"].is_null() || reply["data"]["level"].is_string(),
            "{reply:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn abort_retry_reports_whether_an_operation_was_active() {
        let messages = dispatch("{\"id\":1,\"type\":\"abort_retry\"}\n").await;
        let reply = replies(&messages)[0];
        assert_eq!(reply["success"], true, "{reply:?}");
        // Nothing is running, so there is nothing to abort.
        assert_eq!(reply["data"]["aborted"], false);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn bash_response_carries_the_process_exit_status_and_output() {
        let messages = dispatch("{\"id\":1,\"type\":\"bash\",\"command\":\"exit 1\"}\n").await;
        let reply = replies(&messages)[0];
        // The tool surfaces a non-zero exit as a tool error, which reaches the
        // client as a successful command whose output describes the failure.
        assert_eq!(reply["command"], "bash", "{reply:?}");
        // A non-zero exit reports the status; the text may live in `output`
        // (what the command printed) or in `error` (the tool's status line).
        let text = format!(
            "{} {}",
            reply["data"]["output"].as_str().unwrap_or_default(),
            reply["data"]["error"].as_str().unwrap_or_default()
        );
        assert!(
            text.to_lowercase().contains("exit") || text.contains('1'),
            "expected the failure to be reported, got: {text:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn abort_bash_is_accepted() {
        let messages = dispatch("{\"id\":1,\"type\":\"abort_bash\"}\n").await;
        let reply = replies(&messages)[0];
        assert_eq!(reply["success"], true, "{reply:?}");
    }
}

/// Session-mutating commands need a durable session, so they get their own
/// harness: the in-memory one above cannot fork or clone.
mod parity_sessions {
    use super::*;
    use std::time::Duration;

    const LIMIT: Duration = Duration::from_secs(20);

    /// A harness on a real JSONL session under `dir`, plus the args that point
    /// at the same directory.
    async fn durable(dir: &std::path::Path) -> (AgentHarness, crate::args::Args) {
        let dir = dir.to_path_buf();
        std::fs::create_dir_all(&dir).unwrap();
        let session = crate::session::build_session(
            &crate::session::SessionSelection::New {
                dir: dir.clone(),
                name: None,
            },
            "/tmp",
        )
        .await
        .expect("create session");

        let provider = tom_ai::providers::faux::FauxProvider::new(
            tom_ai::providers::faux::FauxScript::new().with_text("reply"),
        );
        let options = tom_harness::types::AgentHarnessOptions {
            model: provider.default_model().clone(),
            models: vec![provider],
            session,
            ..Default::default()
        };
        let harness = AgentHarness::create(options).await.unwrap();

        let args = crate::args::Args {
            session_dir: Some(dir),
            ..Default::default()
        };
        (harness, args)
    }

    async fn dispatch(
        harness: &AgentHarness,
        args: &crate::args::Args,
        frames: &str,
    ) -> Vec<serde_json::Value> {
        let output = RpcOutput::default();
        let code = tokio::time::timeout(
            LIMIT,
            rpc_with_io(
                harness,
                None,
                vec![],
                tom_extensions::UiDialogMailbox::new(),
                None,
                crate::session::SessionEventGate::none(),
                crate::args::RpcMessageUpdates::default(),
                tokio::io::BufReader::new(frames.as_bytes()),
                output.clone(),
                args.clone(),
                std::path::PathBuf::from("/tmp"),
            ),
        )
        .await
        .expect("dispatcher must finish");
        assert_eq!(code, 0);
        output.messages()
    }

    fn reply(messages: &[serde_json::Value], id: i64) -> serde_json::Value {
        messages
            .iter()
            .find(|message| message["type"] == "response" && message["id"] == id)
            .cloned()
            .unwrap_or_else(|| panic!("no reply for id {id}: {messages:?}"))
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn clone_starts_a_fresh_session_that_is_not_the_old_one() {
        let dir = tempfile::tempdir().unwrap();
        let (harness, args) = durable(dir.path()).await;
        let before = harness.session().storage().metadata().id.clone();

        let messages = dispatch(&harness, &args, "{\"id\":1,\"type\":\"clone\"}\n").await;
        assert_eq!(reply(&messages, 1)["success"], true, "{messages:?}");

        let after = harness.session().storage().metadata().id.clone();
        assert_ne!(before, after, "clone must switch to a new session");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn new_session_switches_to_an_empty_session() {
        let dir = tempfile::tempdir().unwrap();
        let (harness, args) = durable(dir.path()).await;
        let before = harness.session().storage().metadata().id.clone();

        let messages = dispatch(&harness, &args, "{\"id\":1,\"type\":\"new_session\"}\n").await;
        let response = reply(&messages, 1);
        assert_eq!(response["success"], true, "{messages:?}");
        assert_eq!(response["data"]["cancelled"], false);

        let after = harness.session().storage().metadata().id.clone();
        assert_ne!(before, after);
        let entries = harness.find_entries(&EntryQuery::default()).await.unwrap();
        assert!(
            entries.is_empty(),
            "a new session starts empty: {entries:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn fork_moves_the_leaf_to_the_requested_message() {
        let dir = tempfile::tempdir().unwrap();
        let (harness, args) = durable(dir.path()).await;

        // Give the session one user turn to fork from.
        harness
            .lane("main")
            .prompt_text("hello", Vec::new())
            .await
            .unwrap();
        let entries = harness
            .find_entries(&EntryQuery {
                order: Some(EntryOrder::OldestFirst),
                ..Default::default()
            })
            .await
            .unwrap();
        let user_entry = entries
            .iter()
            .find(|entry| {
                serde_json::to_value(entry)
                    .ok()
                    .and_then(|data| data.get("message").cloned())
                    .and_then(|message| message.get("role").cloned())
                    .and_then(|role| role.as_str().map(str::to_owned))
                    .as_deref()
                    == Some("user")
            })
            .expect("a user entry");

        let frames = format!(
            "{{\"id\":1,\"type\":\"fork\",\"entryId\":\"{}\"}}\n",
            user_entry.id()
        );
        let messages = dispatch(&harness, &args, &frames).await;
        assert_eq!(reply(&messages, 1)["success"], true, "{messages:?}");

        let leaf = harness.get_leaf_id().await.unwrap();
        assert_eq!(
            leaf.as_deref(),
            Some(user_entry.id()),
            "fork must leave the leaf at the chosen message"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn switch_session_rejects_an_unknown_path() {
        let dir = tempfile::tempdir().unwrap();
        let (harness, args) = durable(dir.path()).await;
        let messages = dispatch(
            &harness,
            &args,
            "{\"id\":1,\"type\":\"switch_session\",\"sessionPath\":\"no-such-session\"}\n",
        )
        .await;
        assert_eq!(reply(&messages, 1)["success"], false, "{messages:?}");
    }

    /// The four session-replacing commands honor `session_before_switch` /
    /// `session_before_fork`: the response must be `success: true` with
    /// `cancelled: true` rather than an error, because the client asked for
    /// something the host declined -- it did not fail.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_vetoed_session_replacement_reports_cancelled_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let (harness, args) = durable(dir.path()).await;
        let before = harness.session().storage().metadata().id.clone();

        // A gate whose extension refuses every switch and fork.
        let gate = crate::session::SessionEventGate::new(std::sync::Arc::new(
            std::sync::Mutex::new(denying_session_extension()),
        ));

        for frame in [
            r#"{"id":1,"type":"new_session"}"#,
            r#"{"id":1,"type":"clone"}"#,
        ] {
            let output = RpcOutput::default();
            let code = tokio::time::timeout(
                LIMIT,
                rpc_with_io(
                    &harness,
                    None,
                    vec![],
                    tom_extensions::UiDialogMailbox::new(),
                    None,
                    gate.clone(),
                    crate::args::RpcMessageUpdates::default(),
                    tokio::io::BufReader::new(format!("{frame}\n").as_bytes()),
                    output.clone(),
                    args.clone(),
                    std::path::PathBuf::from("."),
                ),
            )
            .await
            .expect("dispatcher must finish");
            assert_eq!(code, 0);
            let messages = output.messages();
            let response = reply(&messages, 1);
            assert_eq!(response["success"], true, "{messages:?}");
            assert_eq!(
                response["data"]["cancelled"], true,
                "a veto must surface as cancelled: {messages:?}"
            );
            assert!(
                response["data"]["reason"]
                    .as_str()
                    .is_some_and(|reason| reason.contains("vetoed")),
                "the reason names the vetoing extension: {messages:?}"
            );
        }

        // The session did not change: a veto costs nothing.
        assert_eq!(
            harness.session().storage().metadata().id,
            before,
            "a vetoed switch must leave the session in place"
        );
    }

    /// Build an extension session with one handler that aborts every lifecycle
    /// event it subscribes to.
    fn denying_session_extension() -> tom_extensions::ExtensionSession {
        extern "C" fn deny(
            _ev: tom_plugin_sdk::StablePluginEvent,
            _ud: *mut std::ffi::c_void,
        ) -> i32 {
            tom_plugin_sdk::EVENT_HANDLER_ABORT
        }
        let mut registry = tom_extensions::ExtensionRegistry::new();
        for tag in [
            tom_plugin_sdk::EventTag::SessionBeforeSwitch,
            tom_plugin_sdk::EventTag::SessionBeforeFork,
        ] {
            registry.register_event_handler(
                "guard-ext".to_string(),
                tag,
                deny,
                std::ptr::null_mut(),
            );
        }
        tom_extensions::ExtensionSession::for_registry(registry)
    }
}
