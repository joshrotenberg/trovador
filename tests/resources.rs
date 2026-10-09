use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use tosi::{
    backend::CliRunner,
    manager::Manager,
    server::{DELEGATION_SKILL_URI, router},
    types::{Backend, RunOutput, RunSpec, Runner},
};
use tower_mcp::testing::TestClient;

struct NoWorkers;

#[async_trait]
impl Runner for NoWorkers {
    async fn run(&self, _spec: RunSpec, _cancel: CancellationToken) -> Result<RunOutput, String> {
        panic!("resource discovery must not start provider workers");
    }
}

#[tokio::test]
async fn standalone_server_lists_and_reads_the_canonical_plugin_skill() {
    let manager = Manager::new(Arc::new(NoWorkers), Backend::Claude, 1);
    let mut client = TestClient::from_router(router(manager, Arc::new(CliRunner::new())));
    let initialized = client.initialize().await;
    assert_eq!(initialized["serverInfo"]["name"], "tosi");
    assert!(initialized["capabilities"].get("resources").is_some());
    let instructions = initialized["instructions"].as_str().unwrap();
    assert!(instructions.contains(DELEGATION_SKILL_URI));
    assert!(instructions.contains("resources/read"));

    let resources = client.list_resources().await;
    assert_eq!(resources.len(), 5);
    let canonical_skill = include_str!("../resources/skills/delegate/SKILL.md");
    let expected = [
        (
            "delegate",
            "skill://tosi/delegate/SKILL.md",
            canonical_skill,
            None,
        ),
        (
            "parallel-tasks",
            "skill://tosi/delegate/references/parallel-tasks.md",
            include_str!("../resources/skills/delegate/references/parallel-tasks.md"),
            Some("references/parallel-tasks.md"),
        ),
        (
            "implementation-review",
            "skill://tosi/delegate/references/implementation-review.md",
            include_str!("../resources/skills/delegate/references/implementation-review.md"),
            Some("references/implementation-review.md"),
        ),
        (
            "follow-up",
            "skill://tosi/delegate/references/follow-up.md",
            include_str!("../resources/skills/delegate/references/follow-up.md"),
            Some("references/follow-up.md"),
        ),
        (
            "model-effort",
            "skill://tosi/delegate/references/model-effort.md",
            include_str!("../resources/skills/delegate/references/model-effort.md"),
            Some("references/model-effort.md"),
        ),
    ];
    for (name, uri, canonical_content, local_reference) in expected {
        let resource = resources
            .iter()
            .find(|resource| resource["uri"] == uri)
            .unwrap_or_else(|| panic!("resource {uri} must be discoverable"));
        assert_eq!(resource["name"], name);
        assert_eq!(resource["mimeType"], "text/markdown");
        assert!(
            resource["description"]
                .as_str()
                .unwrap()
                .contains("plugin package")
        );
        let read = client.read_resource(uri).await;
        assert_eq!(read.contents.len(), 1);
        let content = &read.contents[0];
        assert_eq!(content.uri, uri);
        assert_eq!(content.mime_type.as_deref(), Some("text/markdown"));
        assert_eq!(content.text.as_deref(), Some(canonical_content));
        if let Some(reference) = local_reference {
            // Both plugin-local navigation and standalone resource discovery
            // must lead from the concise core skill to each available pattern.
            assert!(canonical_skill.contains(&format!("]({reference})")));
            assert!(canonical_skill.contains(uri));
        }
    }
    assert!(instructions.contains("only the relevant linked pattern"));
    assert!(canonical_skill.contains("not inherit this conversation"));
    assert!(canonical_skill.contains("bypass a denied action"));

    // Behavioral instructions are a resource, not an extra tool.
    let mut tools: Vec<_> = client
        .list_tools()
        .await
        .into_iter()
        .map(|tool| tool["name"].as_str().unwrap().to_owned())
        .collect();
    tools.sort();
    assert_eq!(
        tools,
        [
            "cancel_task",
            "check_environment",
            "continue_task",
            "spawn_task",
            "wait_tasks"
        ]
    );
}
