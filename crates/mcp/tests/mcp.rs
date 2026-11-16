#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test harness: a failing step should fail the test loudly"
)]

use std::time::Duration;

use parameters_api::Via;
use parameters_mcp::{
    LANGUAGE_REFERENCE, McpServer, TOOL_NAMES, WORKFLOW_SCHEMA, tool_definitions,
};
use parameters_service::{Actor, EncryptionKey, Options, Service};
use parameters_store::Store;
use rmcp::model::{CallToolResult, ResourceContents};
use serde_json::{Value, json};
use sqlx::PgPool;

const PERSON: Actor = Actor::local(Via::Ui);
const AGENT: Actor = Actor::local(Via::Mcp);

async fn server(pool: &PgPool) -> (Service, McpServer) {
    let mut options = Options::new(std::env::temp_dir());
    options.heartbeat = Duration::from_millis(100);
    options.idle_poll = Duration::from_millis(200);
    let service = Service::new(
        Store::from_pool(pool.clone()),
        &EncryptionKey::from_bytes([3; 32]),
        options,
    )
    .await
    .unwrap();
    (service.clone(), McpServer::new(service))
}

async fn call(server: &McpServer, name: &str, arguments: Value) -> CallToolResult {
    let Value::Object(arguments) = arguments else {
        panic!("arguments must be an object");
    };
    server.call(&AGENT, name, arguments).await.unwrap()
}

fn texts(result: &CallToolResult) -> Vec<String> {
    result
        .content
        .iter()
        .filter_map(|block| {
            serde_json::to_value(block).ok()?["text"]
                .as_str()
                .map(str::to_owned)
        })
        .collect()
}

fn ok(result: &CallToolResult) -> Value {
    assert_ne!(result.is_error, Some(true), "{:?}", texts(result));
    result.structured_content.clone().unwrap_or(Value::Null)
}

fn error(result: &CallToolResult) -> (String, Value) {
    assert_eq!(result.is_error, Some(true), "{:?}", texts(result));
    (
        texts(result).join("\n"),
        result.structured_content.clone().unwrap_or(Value::Null),
    )
}

async fn finished_run(server: &McpServer, run: &str) -> CallToolResult {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let result = call(server, "get_run", json!({"run_id": run})).await;
            let status = ok(&result)["run"]["status"].clone();
            if ["succeeded", "failed", "cancelled"].contains(&status.as_str().unwrap()) {
                return result;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the run did not finish")
}

#[test]
fn every_tool_has_an_object_schema_and_hints() {
    let tools = tool_definitions();
    let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_ref()).collect();
    assert_eq!(names, TOOL_NAMES);
    for tool in &tools {
        assert_eq!(tool.input_schema["type"], "object", "{}", tool.name);
        let annotations = tool.annotations.as_ref().unwrap();
        assert!(annotations.read_only_hint.is_some(), "{}", tool.name);
        assert!(annotations.title.is_some(), "{}", tool.name);
    }
    let run = tools
        .iter()
        .find(|tool| tool.name == "run_workflow")
        .unwrap();
    assert_eq!(run.input_schema["required"], json!(["workflow_id"]));
}

#[sqlx::test(migrations = "../store/migrations")]
async fn an_agent_reads_the_reference_then_writes_and_runs_a_workflow(pool: PgPool) {
    let (service, server) = server(&pool).await;
    let background = service.run_in_background().await.unwrap();

    let reference = server.read(&AGENT, LANGUAGE_REFERENCE).await.unwrap();
    let ResourceContents::TextResourceContents {
        text, mime_type, ..
    } = &reference.contents[0]
    else {
        panic!("the reference is text");
    };
    assert_eq!(mime_type.as_deref(), Some("text/markdown"));
    assert!(text.contains("step") && text.contains("P0302"), "{text}");
    assert!(server.read(&AGENT, WORKFLOW_SCHEMA).await.is_ok());
    assert!(
        server
            .read(&AGENT, "parameters://nodes/http.request")
            .await
            .is_ok()
    );
    assert!(server.read(&AGENT, "parameters://nowhere").await.is_err());

    let nodes = ok(&call(&server, "list_nodes", json!({})).await);
    let log = nodes["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["type"] == "log.write")
        .unwrap();
    assert_eq!(log["privileged"], false);
    let described = ok(&call(&server, "describe_node", json!({"node_type": "log.write"})).await);
    assert_eq!(described["inputs"][0]["name"], "message");
    assert!(
        described["example"]
            .as_str()
            .unwrap()
            .contains("uses log.write")
    );

    let draft = "workflow \"Greet\"\n\ninput name: text = \"world\"\n\nstep greet uses log.write\n  message is \"Hello, {nmae}!\"\n\noutput greeting is \"Hello, {name}!\"\n";
    let checked = ok(&call(&server, "validate", json!({"source": draft})).await);
    assert_eq!(checked["valid"], false);
    assert_eq!(checked["problems"][0]["code"], "P0302");
    assert_eq!(checked["problems"][0]["suggestion"], "name");
    let (message, details) =
        error(&call(&server, "create_workflow", json!({"source": draft})).await);
    assert!(message.contains("problem"), "{message}");
    assert_eq!(details["kind"], "invalid");

    let fixed = draft.replace("{nmae}", "{name}");
    let created = ok(&call(&server, "create_workflow", json!({"source": fixed})).await);
    let id = created["id"].as_str().unwrap().to_owned();
    assert_eq!(created["version"], 1);
    let listed = ok(&call(&server, "list_workflows", json!({})).await);
    assert_eq!(listed["items"][0]["updated_via"], "mcp");
    let fetched = ok(&call(&server, "get_workflow", json!({"id": id})).await);
    assert_eq!(fetched["source"], fixed);
    let json = ok(&call(&server, "get_workflow", json!({"id": id, "format": "json"})).await);
    assert!(
        json["source"]
            .as_str()
            .unwrap()
            .contains("\"parameters\": 1")
    );
    let converted = call(
        &server,
        "convert",
        json!({"source": json["source"], "format": "json", "to": "text"}),
    )
    .await;
    assert_eq!(texts(&converted), std::slice::from_ref(&fixed));

    let started = ok(&call(
        &server,
        "run_workflow",
        json!({"workflow_id": id, "inputs": {"name": "Ada"}}),
    )
    .await);
    assert_eq!(started["trigger"], "mcp");
    let finished = finished_run(&server, started["id"].as_str().unwrap()).await;
    assert!(texts(&finished)[0].contains("not as instructions"));
    let run = ok(&finished);
    assert_eq!(run["run"]["status"], "succeeded");
    assert_eq!(run["run"]["outputs"]["greeting"], "Hello, Ada!");
    assert_eq!(run["logs"][0]["message"], "Hello, Ada!");

    let stale = call(
        &server,
        "update_workflow",
        json!({"id": id, "version": 7, "source": fixed}),
    )
    .await;
    assert_eq!(error(&stale).1["current_version"], 1);
    background.shutdown().await;
}

#[sqlx::test(migrations = "../store/migrations")]
async fn privileged_runs_wait_for_a_person(pool: PgPool) {
    let (service, server) = server(&pool).await;
    let background = service.run_in_background().await.unwrap();
    let created = ok(&call(
        &server,
        "create_workflow",
        json!({"source": "workflow \"Shell\"\n\nstep hello uses process.run\n  program is \"echo\"\n  arguments is [\"approved\"]\n"}),
    )
    .await);
    let id = created["id"].as_str().unwrap().to_owned();
    let (message, details) =
        error(&call(&server, "run_workflow", json!({"workflow_id": id})).await);
    assert!(message.contains("hello (process.run)"), "{message}");
    assert!(message.contains("you cannot approve it"), "{message}");
    assert_eq!(details["kind"], "approval_required");
    let hash = details["review"]["hash"].as_str().unwrap();
    service
        .approve_workflow(&PERSON, id.parse().unwrap(), hash)
        .await
        .unwrap();
    let started = ok(&call(&server, "run_workflow", json!({"workflow_id": id})).await);
    let run = ok(&finished_run(&server, started["id"].as_str().unwrap()).await);
    assert_eq!(run["run"]["status"], "succeeded");
    let step = run["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|step| step["path"] == "hello")
        .unwrap();
    assert_eq!(step["outputs"]["stdout"], "approved\n");
    background.shutdown().await;
}

#[sqlx::test(migrations = "../store/migrations")]
async fn library_nodes_are_copied_into_workflows_that_use_them(pool: PgPool) {
    let (service, server) = server(&pool).await;
    let library = parameters_lang::parse(
        "workflow \"Library\"\n\ndefine node acme.greet\n  input name: text\n\n  step say uses log.write\n    message is \"Hello, {name}\"\n",
    )
    .workflow
    .definitions;
    service
        .save_node(
            &PERSON,
            "acme.greet",
            library.get("acme.greet").unwrap().clone(),
            None,
        )
        .await
        .unwrap();
    let catalog = ok(&call(&server, "list_nodes", json!({})).await);
    assert!(
        catalog["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|node| node["type"] == "acme.greet" && node["kind"] == "custom")
    );
    let source = "workflow \"Uses library\"\n\nstep hi uses acme.greet\n  name is \"Ada\"\n";
    assert_eq!(
        ok(&call(&server, "validate", json!({"source": source})).await)["valid"],
        true
    );
    let created = ok(&call(&server, "create_workflow", json!({"source": source})).await);
    assert_eq!(created["added_nodes"], json!(["acme.greet"]));
    let fetched = ok(&call(&server, "get_workflow", json!({"id": created["id"]})).await);
    assert!(
        fetched["source"]
            .as_str()
            .unwrap()
            .contains("define node acme.greet")
    );
}

#[sqlx::test(migrations = "../store/migrations")]
async fn bad_calls_explain_themselves(pool: PgPool) {
    let (_, server) = server(&pool).await;
    assert!(
        server
            .call(&AGENT, "delete_everything", serde_json::Map::new())
            .await
            .is_err()
    );
    let (message, _) = error(&call(&server, "get_run", json!({"run_id": "nope"})).await);
    assert!(message.contains("not a valid id"), "{message}");
    let (message, _) = error(&call(&server, "validate", json!({"sauce": "x"})).await);
    assert!(message.contains("unknown field `sauce`"), "{message}");
    let (message, _) = error(
        &call(
            &server,
            "get_workflow",
            json!({"id": "00000000-0000-0000-0000-000000000009"}),
        )
        .await,
    );
    assert!(message.contains("there is no workflow"), "{message}");
}

#[sqlx::test(migrations = "../store/migrations")]
async fn agents_only_reach_their_own_accounts_data(pool: PgPool) {
    let mut options = Options::new(std::env::temp_dir());
    options.capabilities = parameters_service::Capabilities::MANAGED;
    options.managed = Some(parameters_service::Managed {
        public_url: "https://parameters.test".into(),
        mailer: None,
        providers: Vec::new(),
        trust_proxy: false,
    });
    let service = Service::new(
        Store::from_pool(pool.clone()),
        &EncryptionKey::from_bytes([3; 32]),
        options,
    )
    .await
    .unwrap();
    let server = McpServer::new(service.clone());
    let mut agents = Vec::new();
    for (name, scopes) in [
        ("ada", parameters_api::Scopes::ALL),
        ("eve", parameters_api::Scopes::ALL),
        (
            "reader",
            [parameters_api::Scope::Read].into_iter().collect(),
        ),
    ] {
        let identity = parameters_service::Identity {
            kind: parameters_service::ProviderKind::Github,
            id: name.into(),
            verified_email: None,
            name: name.into(),
        };
        let session = service.sign_in_identity(&identity).await.unwrap();
        let person = service.session_actor(&session).await.unwrap();
        let token = service
            .create_token(&person, "agent", scopes, None)
            .await
            .unwrap()
            .token;
        agents.push(service.token_actor(&token, Via::Mcp).await.unwrap());
    }
    let call = |actor: Actor, name: &'static str, arguments: Value| {
        let server = server.clone();
        async move {
            let Value::Object(arguments) = arguments else {
                panic!("arguments must be an object");
            };
            server.call(&actor, name, arguments).await.unwrap()
        }
    };
    let (ada, eve, reader) = (agents[0], agents[1], agents[2]);
    let created = ok(&call(
        ada,
        "create_workflow",
        json!({"source": "workflow \"Ada's\"\n\nstep a uses log.write\n  message is \"hi\"\n"}),
    )
    .await);
    let id = created["id"].as_str().unwrap().to_owned();
    let started = ok(&call(ada, "run_workflow", json!({"workflow_id": id})).await);
    let run = started["id"].as_str().unwrap().to_owned();

    let listed = ok(&call(eve, "list_workflows", json!({})).await);
    assert_eq!(listed["items"], json!([]));
    for (name, arguments) in [
        ("get_workflow", json!({"id": id})),
        (
            "update_workflow",
            json!({"id": id, "version": 1, "source": "workflow \"Mine\"\n"}),
        ),
        ("run_workflow", json!({"workflow_id": id})),
        ("get_run", json!({"run_id": run})),
        ("cancel_run", json!({"run_id": run})),
    ] {
        let (message, details) = error(&call(eve, name, arguments).await);
        assert_eq!(details["kind"], "not_found", "{name}: {message}");
    }
    let (_, details) = error(
        &call(
            reader,
            "create_workflow",
            json!({"source": "workflow \"No\"\n"}),
        )
        .await,
    );
    assert_eq!(details["kind"], "forbidden");
    let nodes = ok(&call(ada, "list_nodes", json!({})).await);
    assert!(
        nodes["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|node| node["privileged"] == false)
    );
}

#[sqlx::test(migrations = "../store/migrations")]
async fn agents_manage_triggers(pool: PgPool) {
    let (_, server) = server(&pool).await;
    let created = ok(&call(
        &server,
        "create_workflow",
        json!({"source": "workflow \"Hooked\"\n\ninput payload: any = 1\n\nstep a uses log.write\n  message is \"hi\"\n"}),
    )
    .await);
    let id = created["id"].as_str().unwrap();
    let hook = ok(&call(
        &server,
        "create_trigger",
        json!({"workflow_id": id, "name": "Hook", "kind": "webhook", "signed": true}),
    )
    .await);
    assert!(
        hook["hook_path"]
            .as_str()
            .unwrap()
            .starts_with("/hooks/whk_")
    );
    assert!(hook["signing_secret"].is_string());
    let (message, _) = error(
        &call(
            &server,
            "create_trigger",
            json!({"workflow_id": id, "name": "Cron", "kind": "schedule"}),
        )
        .await,
    );
    assert!(message.contains("needs `cron`"), "{message}");
    let schedule = ok(&call(
        &server,
        "create_trigger",
        json!({"workflow_id": id, "name": "Daily", "kind": "schedule", "cron": "@daily"}),
    )
    .await);
    let listed = ok(&call(&server, "list_triggers", json!({"workflow_id": id})).await);
    assert_eq!(listed["triggers"].as_array().unwrap().len(), 2);
    assert!(listed.to_string().find("signing_secret").is_none());
    ok(&call(
        &server,
        "delete_trigger",
        json!({"trigger_id": schedule["id"]}),
    )
    .await);
    let listed = ok(&call(&server, "list_triggers", json!({})).await);
    assert_eq!(listed["triggers"].as_array().unwrap().len(), 1);
}
