#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test harness: a failing step should fail the test loudly"
)]

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use parameters_api::{
    ApiError, ImportStatus, LogLine, Resolution, RunRecord, RunState, Source, StepRecord,
    StepState, Via,
};
use parameters_core::{Definition, OrderedMap};
use parameters_service::{Actor, EncryptionKey, Options, Service};
use parameters_store::Store;
use serde_json::{Map, json};
use sqlx::PgPool;
use uuid::Uuid;

const UI: Actor = Actor::local(Via::Ui);
const MCP: Actor = Actor::local(Via::Mcp);
const PATIENCE: Duration = Duration::from_secs(20);

fn options() -> Options {
    let mut options = Options::new(std::env::temp_dir());
    options.heartbeat = Duration::from_millis(100);
    options.stale_after = Duration::from_millis(600);
    options.idle_poll = Duration::from_millis(200);
    options.retry_delay = Some(Duration::from_millis(1));
    options
}

async fn service(pool: &PgPool) -> Service {
    Service::new(
        Store::from_pool(pool.clone()),
        &EncryptionKey::from_bytes([7; 32]),
        options(),
    )
    .await
    .unwrap()
}

fn text(source: &str) -> Source {
    Source::Text(source.to_owned())
}

struct Finished {
    run: RunRecord,
    steps: HashMap<String, StepRecord>,
    logs: Vec<LogLine>,
    updates: usize,
}

async fn finished(service: &Service, run: Uuid) -> Finished {
    let mut watch = service.watch_run(&UI, run).await.unwrap();
    let mut steps = HashMap::new();
    let mut logs = Vec::new();
    let mut updates = 0;
    tokio::time::timeout(PATIENCE, async {
        while let Some(update) = watch.next().await {
            let update = update.unwrap();
            updates += 1;
            for step in update.steps {
                steps.insert(step.path.clone(), step);
            }
            logs.extend(update.logs);
            if update.run.summary.status.is_finished() {
                return update.run;
            }
        }
        panic!("the watch ended before the run finished");
    })
    .await
    .map(|run| Finished {
        run,
        steps,
        logs,
        updates,
    })
    .expect("the run did not finish in time")
}

async fn wait_for(service: &Service, run: Uuid, status: RunState) {
    tokio::time::timeout(PATIENCE, async {
        loop {
            if service.get_run(&UI, run).await.unwrap().run.summary.status == status {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("the run never became {}", status.name()));
}

#[sqlx::test(migrations = "../store/migrations")]
async fn workflows_are_versioned_paged_and_deleted(pool: PgPool) {
    let service = service(&pool).await;
    let created = service
        .create_workflow(
            &MCP,
            &text("workflow \"First\"\n\nstep a uses log.write\n  message is \"hi\"\n"),
        )
        .await
        .unwrap();
    assert_eq!(created.summary.version, 1);
    assert_eq!(created.summary.updated_via, Via::Mcp);
    assert_eq!(
        created.document.layout.len(),
        1,
        "layout is completed on save"
    );
    let id = created.summary.id;
    let updated = service
        .update_workflow(
            &UI,
            id,
            1,
            &text("workflow \"Renamed\"\n\nstep a uses log.write\n  message is \"bye\"\n"),
        )
        .await
        .unwrap();
    assert_eq!(
        (updated.summary.version, updated.summary.name.as_str()),
        (2, "Renamed")
    );
    assert_ne!(updated.hash, created.hash);
    let stale = service
        .update_workflow(&MCP, id, 1, &text("workflow \"Lost\"\n"))
        .await
        .unwrap_err();
    assert!(
        matches!(
            stale,
            ApiError::Conflict {
                current_version: Some(2),
                ..
            }
        ),
        "{stale:?}"
    );
    assert_eq!(
        service.get_workflow(&UI, id).await.unwrap().document.name,
        "Renamed"
    );
    for name in ["Second", "Third"] {
        service
            .create_workflow(
                &UI,
                &Source::Json(format!("{{\"parameters\": 1, \"name\": \"{name}\"}}")),
            )
            .await
            .unwrap();
    }
    let first = service.list_workflows(&UI, Some(2), None).await.unwrap();
    assert_eq!(first.items.len(), 2);
    let rest = service
        .list_workflows(&UI, Some(2), first.next.as_deref())
        .await
        .unwrap();
    assert_eq!(rest.items.len(), 1);
    assert_eq!(rest.next, None);
    let mut names: Vec<String> = first
        .items
        .iter()
        .chain(&rest.items)
        .map(|item| item.name.clone())
        .collect();
    names.sort();
    assert_eq!(names, ["Renamed", "Second", "Third"]);
    assert!(matches!(
        service.list_workflows(&UI, None, Some("garbage")).await,
        Err(ApiError::BadRequest { .. })
    ));
    service.delete_workflow(&UI, id).await.unwrap();
    assert!(matches!(
        service.get_workflow(&UI, id).await,
        Err(ApiError::NotFound { .. })
    ));
    assert!(matches!(
        service.delete_workflow(&UI, id).await,
        Err(ApiError::NotFound { .. })
    ));
}

#[sqlx::test(migrations = "../store/migrations")]
async fn invalid_workflows_are_rejected_with_problems(pool: PgPool) {
    let service = service(&pool).await;
    let error = service
        .create_workflow(
            &MCP,
            &text("workflow \"Broken\"\n\nstep a uses log.write\n  message is chekout.path\n"),
        )
        .await
        .unwrap_err();
    let ApiError::Invalid { problems, .. } = error else {
        panic!("{error:?}");
    };
    assert_eq!(problems[0].code, "P0302");
    assert_eq!(problems[0].range.unwrap().start.line, 4);
    let problems = service.check(&Source::Json(
        "{\"parameters\": 1, \"name\": \"A\", \"stpes\": []}".into(),
    ));
    assert_eq!(problems[0].code, "P0001");
    assert!(
        problems[0].message.contains("stpes"),
        "{}",
        problems[0].message
    );
}

#[sqlx::test(migrations = "../store/migrations")]
async fn runs_stream_live_updates_and_mask_secrets(pool: PgPool) {
    let service = service(&pool).await;
    let background = service.run_in_background().await.unwrap();
    service.set_secret(&UI, "token", "hunter2").await.unwrap();
    service.set_secret(&UI, "API_KEY", "abc123").await.unwrap();
    let secrets = service.list_secrets(&UI).await.unwrap();
    assert_eq!(
        secrets
            .iter()
            .map(|secret| secret.name.as_str())
            .collect::<Vec<_>>(),
        ["API_KEY", "token"]
    );
    let workflow = service
        .create_workflow(
            &UI,
            &text("workflow \"Greet\"\n\ninput token: secret\ninput name: text = \"world\"\n\nstep hello uses log.write\n  message is \"Hello, {name}! {token}\"\n\nstep key uses log.write\n  after hello\n  message is secret \"API_KEY\"\n\noutput greeting is \"{name} {token}\"\n"),
        )
        .await
        .unwrap();
    let id = workflow.summary.id;
    let bad = service
        .start_run(&UI, id, Map::from_iter([("nmae".to_owned(), json!("x"))]))
        .await
        .unwrap_err();
    assert!(
        matches!(&bad, ApiError::BadRequest { message } if message.contains("no input `nmae`")),
        "{bad:?}"
    );
    let typed = service
        .start_run(&UI, id, Map::from_iter([("name".to_owned(), json!(5))]))
        .await
        .unwrap_err();
    assert!(matches!(typed, ApiError::BadRequest { .. }), "{typed:?}");
    let run = service
        .start_run(&UI, id, Map::from_iter([("name".to_owned(), json!("Ada"))]))
        .await
        .unwrap();
    assert_eq!(run.trigger, Via::Ui);
    let done = finished(&service, run.id).await;
    assert_eq!(
        done.run.summary.status,
        RunState::Succeeded,
        "{:?}",
        done.run.errors
    );
    assert_eq!(done.run.outputs["greeting"], "Ada ••••");
    assert_eq!(done.steps["hello"].status, StepState::Succeeded);
    assert_eq!(done.steps["key"].status, StepState::Succeeded);
    let messages: Vec<&str> = done.logs.iter().map(|log| log.message.as_str()).collect();
    assert_eq!(messages, ["Hello, Ada! ••••", "••••"]);
    let everything = serde_json::to_string(&service.get_run(&UI, run.id).await.unwrap()).unwrap();
    assert!(!everything.contains("hunter2") && !everything.contains("abc123"));
    let runs = service.list_runs(&UI, Some(id), None, None).await.unwrap();
    assert_eq!(runs.items.len(), 1);
    service.delete_secret(&UI, "token").await.unwrap();
    let missing = service.start_run(&UI, id, Map::new()).await.unwrap();
    let failed = finished(&service, missing.id).await;
    assert_eq!(failed.run.summary.status, RunState::Failed);
    assert_eq!(
        failed.run.errors,
        ["the secret for the input `token` is not set"]
    );
    background.shutdown().await;
}

#[sqlx::test(migrations = "../store/migrations")]
async fn privileged_workflows_need_a_person_to_approve_each_version(pool: PgPool) {
    let service = service(&pool).await;
    let background = service.run_in_background().await.unwrap();
    let workflow = service
        .create_workflow(
            &MCP,
            &text(
                "workflow \"Shell\"\n\nstep say uses shell.run\n  command is \"echo approved\"\n",
            ),
        )
        .await
        .unwrap();
    let id = workflow.summary.id;
    let blocked = service.start_run(&MCP, id, Map::new()).await.unwrap_err();
    let ApiError::ApprovalRequired { review, .. } = blocked else {
        panic!("{blocked:?}");
    };
    assert_eq!(review.privileged[0].uses, "shell.run");
    assert!(!review.approved);
    assert!(matches!(
        service.approve_workflow(&MCP, id, &review.hash).await,
        Err(ApiError::BadRequest { .. })
    ));
    assert!(matches!(
        service.approve_workflow(&UI, id, &"0".repeat(64)).await,
        Err(ApiError::Conflict { .. })
    ));
    let approved = service
        .approve_workflow(&UI, id, &review.hash)
        .await
        .unwrap();
    assert!(approved.approved);
    let run = service.start_run(&MCP, id, Map::new()).await.unwrap();
    let done = finished(&service, run.id).await;
    assert_eq!(
        done.run.summary.status,
        RunState::Succeeded,
        "{:?}",
        done.run.errors
    );
    assert_eq!(done.steps["say"].outputs["stdout"], "approved\n");
    service
        .update_workflow(
            &MCP,
            id,
            1,
            &text("workflow \"Shell\"\n\nstep say uses shell.run\n  command is \"echo changed\"\n"),
        )
        .await
        .unwrap();
    assert!(matches!(
        service.start_run(&MCP, id, Map::new()).await,
        Err(ApiError::ApprovalRequired { .. })
    ));
    background.shutdown().await;
}

#[sqlx::test(migrations = "../store/migrations")]
async fn runs_can_be_cancelled_while_queued_or_running(pool: PgPool) {
    let service = service(&pool).await;
    let workflow = service
        .create_workflow(
            &UI,
            &text("workflow \"Slow\"\n\nstep pause uses time.wait\n  duration is \"30s\"\n"),
        )
        .await
        .unwrap();
    let id = workflow.summary.id;
    let queued = service.start_run(&UI, id, Map::new()).await.unwrap();
    assert_eq!(
        service.cancel_run(&UI, queued.id).await.unwrap(),
        RunState::Cancelled
    );
    let background = service.run_in_background().await.unwrap();
    let running = service.start_run(&UI, id, Map::new()).await.unwrap();
    wait_for(&service, running.id, RunState::Running).await;
    assert_eq!(
        service.cancel_run(&UI, running.id).await.unwrap(),
        RunState::Running
    );
    let done = finished(&service, running.id).await;
    assert_eq!(done.run.summary.status, RunState::Cancelled);
    assert!(done.run.cancel_requested);
    assert_eq!(
        service
            .get_run(&UI, queued.id)
            .await
            .unwrap()
            .run
            .summary
            .status,
        RunState::Cancelled
    );
    assert!(matches!(
        service.cancel_run(&UI, Uuid::from_u128(5)).await,
        Err(ApiError::NotFound { .. })
    ));
    background.shutdown().await;
}

#[sqlx::test(migrations = "../store/migrations")]
async fn runs_survive_a_server_restart(pool: PgPool) {
    let first = service(&pool).await;
    let workflow = first
        .create_workflow(&UI, &text("workflow \"Long\"\n\nstep pause uses time.wait\n  duration is \"1s\"\n\nstep finish uses log.write\n  after pause\n  message is \"done\"\n"))
        .await
        .unwrap();
    let id = workflow.summary.id;
    let background = first.run_in_background().await.unwrap();
    let graceful = first.start_run(&UI, id, Map::new()).await.unwrap();
    wait_for(&first, graceful.id, RunState::Running).await;
    background.shutdown().await;
    assert_eq!(
        first
            .get_run(&UI, graceful.id)
            .await
            .unwrap()
            .run
            .summary
            .status,
        RunState::Queued
    );
    let store = Store::from_pool(pool.clone());
    let crashed = first.start_run(&UI, id, Map::new()).await.unwrap();
    let dead_worker = Uuid::from_u128(77);
    let claimed = [
        store.claim_run(dead_worker).await.unwrap().unwrap().id,
        store.claim_run(dead_worker).await.unwrap().unwrap().id,
    ];
    assert_eq!(claimed, [graceful.id, crashed.id]);
    let second = service(&pool).await;
    let background = second.run_in_background().await.unwrap();
    for run in [graceful.id, crashed.id] {
        let done = finished(&second, run).await;
        assert_eq!(
            done.run.summary.status,
            RunState::Succeeded,
            "{:?}",
            done.run.errors
        );
        assert_eq!(done.steps["finish"].status, StepState::Succeeded);
        assert!(
            done.logs
                .iter()
                .any(|log| log.message.contains("starts again")),
            "{:?}",
            done.logs
        );
        assert!(done.updates >= 1);
    }
    background.shutdown().await;
}

#[sqlx::test(migrations = "../store/migrations")]
async fn runs_that_keep_crashing_are_failed(pool: PgPool) {
    let service = service(&pool).await;
    let store = Store::from_pool(pool.clone());
    let workflow = service
        .create_workflow(
            &UI,
            &text("workflow \"Doomed\"\n\nstep a uses log.write\n  message is \"x\"\n"),
        )
        .await
        .unwrap();
    let run = service
        .start_run(&UI, workflow.summary.id, Map::new())
        .await
        .unwrap();
    for attempt in 1..=3 {
        let claimed = store.claim_run(Uuid::from_u128(77)).await.unwrap().unwrap();
        assert_eq!(claimed.attempts, attempt);
        tokio::time::sleep(Duration::from_millis(30)).await;
        let recovered = store
            .recover_stale_runs(Duration::from_millis(10), 3)
            .await
            .unwrap();
        let expected = if attempt < 3 {
            RunState::Queued
        } else {
            RunState::Failed
        };
        assert_eq!(recovered, [(run.id, expected)]);
    }
    let failed = service.get_run(&UI, run.id).await.unwrap().run;
    assert_eq!(
        failed.errors,
        ["the server stopped during the run too many times"]
    );
    assert!(
        store
            .claim_run(Uuid::from_u128(77))
            .await
            .unwrap()
            .is_none()
    );
}

fn definition(source: &str) -> OrderedMap<Definition> {
    let parsed = parameters_lang::parse(source);
    assert!(!parsed.has_errors(), "{:?}", parsed.diagnostics);
    parsed.workflow.definitions
}

#[sqlx::test(migrations = "../store/migrations")]
async fn the_node_library_saves_renames_and_imports(pool: PgPool) {
    let service = service(&pool).await;
    let library = definition(
        "workflow \"Library\"\n\ndefine node acme.greet\n  input name: text\n\n  step say uses log.write\n    message is \"Hello, {name}\"\n\ndefine node acme.twice\n  step first uses acme.greet\n    name is \"one\"\n\n  step second uses acme.greet\n    name is \"two\"\n",
    );
    let greet = library.get("acme.greet").unwrap().clone();
    let twice = library.get("acme.twice").unwrap().clone();
    assert!(matches!(
        service
            .save_node(&UI, "acme.twice", twice.clone(), None)
            .await,
        Err(ApiError::Invalid { .. })
    ));
    service
        .save_node(&UI, "acme.greet", greet.clone(), None)
        .await
        .unwrap();
    service
        .save_node(&UI, "acme.twice", twice, None)
        .await
        .unwrap();
    assert!(matches!(
        service
            .save_node(&UI, "log.write", greet.clone(), None)
            .await,
        Err(ApiError::BadRequest { .. })
    ));
    assert!(matches!(
        service
            .save_node(&UI, "acme.greet", greet.clone(), None)
            .await,
        Err(ApiError::Conflict {
            current_version: Some(1),
            ..
        })
    ));
    let mut noted = greet.clone();
    noted.note = Some("Says hello".into());
    let saved = service
        .save_node(&UI, "acme.greet", noted, Some(1))
        .await
        .unwrap();
    assert_eq!(
        (saved.summary.version, saved.summary.note.as_deref()),
        (2, Some("Says hello"))
    );
    assert!(matches!(
        service.delete_node(&UI, "acme.greet", 2).await,
        Err(ApiError::BadRequest { .. })
    ));
    let renamed = service
        .rename_node(&UI, "acme.greet", "acme.hello")
        .await
        .unwrap();
    assert_eq!(renamed.summary.type_name, "acme.hello");
    let twice = service.get_node(&UI, "acme.twice").await.unwrap();
    assert!(
        twice
            .definition
            .steps
            .iter()
            .all(|step| step.uses == "acme.hello")
    );
    let names: Vec<String> = service
        .list_nodes(&UI)
        .await
        .unwrap()
        .into_iter()
        .map(|node| node.type_name)
        .collect();
    assert_eq!(names, ["acme.hello", "acme.twice"]);

    let incoming = definition(
        "workflow \"Incoming\"\n\ndefine node acme.hello\n  step other uses time.wait\n    duration is \"1s\"\n\ndefine node acme.fresh\n  step x uses time.wait\n    duration is \"2s\"\n\ndefine node log.write\n  step y uses time.wait\n    duration is \"1s\"\n",
    );
    let plan = service.plan_node_import(&UI, &incoming).await.unwrap();
    let statuses: BTreeMap<&str, &ImportStatus> = plan
        .iter()
        .map(|item| (item.name.as_str(), &item.status))
        .collect();
    assert_eq!(statuses["acme.fresh"], &ImportStatus::New);
    assert!(matches!(
        statuses["acme.hello"],
        ImportStatus::Conflict { .. }
    ));
    assert!(matches!(
        statuses["log.write"],
        ImportStatus::BuiltinClash { .. }
    ));
    let resolutions = BTreeMap::from([
        (
            "acme.hello".to_owned(),
            Resolution::Rename("acme.hello2".into()),
        ),
        (
            "log.write".to_owned(),
            Resolution::Rename("acme.pause".into()),
        ),
    ]);
    service
        .import_nodes(&UI, &incoming, &resolutions)
        .await
        .unwrap();
    let names: Vec<String> = service
        .list_nodes(&UI)
        .await
        .unwrap()
        .into_iter()
        .map(|node| node.type_name)
        .collect();
    assert_eq!(
        names,
        [
            "acme.fresh",
            "acme.hello",
            "acme.hello2",
            "acme.pause",
            "acme.twice"
        ]
    );

    service
        .create_workflow(&UI, &text("workflow \"User\"\n\nstep a uses acme.fresh\n\ndefine node acme.fresh\n  step x uses time.wait\n    duration is \"2s\"\n"))
        .await
        .unwrap();
    let users = service.node_users(&UI, "acme.fresh").await.unwrap();
    assert_eq!(users.len(), 1);
    assert_eq!(service.node_users(&UI, "acme.twice").await.unwrap(), []);
    service.delete_node(&UI, "acme.fresh", 1).await.unwrap();
    assert!(matches!(
        service.get_node(&UI, "acme.fresh").await,
        Err(ApiError::NotFound { .. })
    ));
}
