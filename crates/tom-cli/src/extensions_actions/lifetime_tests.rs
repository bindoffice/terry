use super::*;
use serde_json::json;
use tom_ai::providers::faux::{FauxProvider, FauxScript};
use tom_harness::types::AgentHarnessOptions;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn expired_reverse_bindings_fail_closed_but_still_reclaim_their_children() {
    let (host, cell) = HarnessActionHost::new_empty(
        vec![],
        std::env::temp_dir(),
        Handle::current(),
        BTreeMap::new(),
        tom_extensions::ToolCallContext::default(),
    );
    assert!(host
        .get_execution_context(json!({}))
        .await
        .unwrap_err()
        .contains("before harness"));
    let provider = FauxProvider::new(
        FauxScript::new()
            .with_text("slow")
            .with_tokens_per_second(0.1),
    );
    let parent = AgentHarness::create(AgentHarnessOptions {
        model: provider.default_model().clone(),
        models: vec![provider],
        ..Default::default()
    })
    .await
    .unwrap();
    HarnessActionHost::set_harness(&cell, Arc::new(parent.clone()));
    assert!(host.get_execution_context(json!({})).await.is_ok());
    let child = host
        .child_agent(7, json!({"op":"start","task":"report"}))
        .await
        .unwrap();
    let id = &child["childId"];
    drop(parent);
    assert!(host
        .get_execution_context(json!({}))
        .await
        .unwrap_err()
        .contains("after harness"));
    assert!(host
        .child_agent(7, json!({"op":"start","task":"stale"}))
        .await
        .unwrap_err()
        .contains("after harness"));
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let value = host
                .child_agent(7, json!({"op":"poll","childId":id}))
                .await
                .unwrap();
            if value["status"] == "failed" {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    assert!(host
        .child_agent(8, json!({"op":"dispose","childId":id}))
        .await
        .is_err());
    host.child_agent(7, json!({"op":"dispose","childId":id}))
        .await
        .unwrap();
    assert!(host
        .child_agent(7, json!({"op":"poll","childId":id}))
        .await
        .is_err());
}
