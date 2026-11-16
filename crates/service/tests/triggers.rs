#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::assert_is_empty,
    clippy::format_collect,
    reason = "test harness: a failing step should fail the test loudly"
)]

use std::time::Duration;

use parameters_api::{ApiError, NewTrigger, RunState, Source, TriggerKind, Via};
use parameters_service::{Actor, Capabilities, EncryptionKey, Managed, Options, Service};
use parameters_store::Store;
use ring::hmac;
use serde_json::{Map, json};
use sqlx::PgPool;
use uuid::Uuid;

const PERSON: Actor = Actor::local(Via::Ui);

async fn service(pool: &PgPool, capabilities: Capabilities) -> Service {
    let mut options = Options::new(std::env::temp_dir());
    options.heartbeat = Duration::from_millis(100);
    options.idle_poll = Duration::from_millis(200);
    options.capabilities = capabilities;
    if capabilities == Capabilities::MANAGED {
        options.managed = Some(Managed {
            public_url: "https://parameters.test".into(),
            mailer: None,
            providers: Vec::new(),
            trust_proxy: false,
        });
    }
    Service::new(
        Store::from_pool(pool.clone()),
        &EncryptionKey::from_bytes([9; 32]),
        options,
    )
    .await
    .unwrap()
}

async fn workflow(service: &Service, actor: &Actor, source: &str) -> Uuid {
    service
        .create_workflow(actor, &Source::Text(source.to_owned()))
        .await
        .unwrap()
        .record
        .summary
        .id
}

async fn finished(service: &Service, run: Uuid) -> parameters_api::RunUpdate {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let update = service.get_run(&PERSON, run).await.unwrap();
            if update.run.summary.status.is_finished() {
                return update;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the run did not finish")
}

fn token(path: &str) -> &str {
    path.strip_prefix("/hooks/").unwrap()
}

const HOOKED: &str = "workflow \"Hooked\"\n\ninput payload: object\n\nstep say uses log.write\n  message is \"Got {payload.event}\"\n";

#[sqlx::test(migrations = "../store/migrations")]
async fn webhooks_start_runs_with_their_payload(pool: PgPool) {
    let service = service(&pool, Capabilities::LOCAL).await;
    let background = service.run_in_background().await.unwrap();
    let id = workflow(&service, &PERSON, HOOKED).await;
    let created = service
        .create_trigger(
            &PERSON,
            id,
            "Deploys",
            NewTrigger::Webhook { signed: false },
            Map::new(),
        )
        .await
        .unwrap();
    assert_eq!(created.signing_secret, None);
    let path = created.hook_path.clone().unwrap();
    assert!(path.starts_with("/hooks/whk_"));
    let run = service
        .fire_webhook(token(&path), br#"{"event": "push"}"#, None)
        .await
        .unwrap();
    assert_eq!(run.trigger, Via::Webhook);
    let done = finished(&service, run.id).await;
    assert_eq!(
        done.run.summary.status,
        RunState::Succeeded,
        "{:?}",
        done.run.errors
    );
    assert_eq!(done.logs[0].message, "Got push");
    let listed = service.list_triggers(&PERSON, Some(id)).await.unwrap();
    assert!(listed[0].last_fired_at.is_some());
    assert_eq!(listed[0].kind, TriggerKind::Webhook { signed: false });

    let wrong = service.fire_webhook(token(&path), b"not json", None).await;
    assert!(
        matches!(wrong, Err(ApiError::BadRequest { .. })),
        "{wrong:?}"
    );
    assert!(
        service.list_triggers(&PERSON, Some(id)).await.unwrap()[0]
            .last_error
            .as_deref()
            .is_some_and(|error| error.contains("payload"))
    );
    assert!(matches!(
        service.fire_webhook("whk_unknown", b"{}", None).await,
        Err(ApiError::NotFound { .. })
    ));
    service
        .set_trigger_enabled(&PERSON, created.summary.id, false)
        .await
        .unwrap();
    assert!(matches!(
        service.fire_webhook(token(&path), b"{}", None).await,
        Err(ApiError::NotFound { .. })
    ));
    service
        .delete_trigger(&PERSON, created.summary.id)
        .await
        .unwrap();
    assert!(
        service
            .list_triggers(&PERSON, None)
            .await
            .unwrap()
            .is_empty()
    );
    background.shutdown().await;
}

#[sqlx::test(migrations = "../store/migrations")]
async fn signed_webhooks_need_a_matching_signature(pool: PgPool) {
    let service = service(&pool, Capabilities::LOCAL).await;
    let id = workflow(&service, &PERSON, HOOKED).await;
    let created = service
        .create_trigger(
            &PERSON,
            id,
            "Signed",
            NewTrigger::Webhook { signed: true },
            Map::new(),
        )
        .await
        .unwrap();
    let secret = created.signing_secret.clone().unwrap();
    let path = created.hook_path.clone().unwrap();
    let body = br#"{"event": "release"}"#;
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
    let signature: String = hmac::sign(&key, body)
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    for bad in [None, Some("sha256=00"), Some("md5=abc")] {
        assert!(
            matches!(
                service.fire_webhook(token(&path), body, bad).await,
                Err(ApiError::Unauthorized { .. })
            ),
            "{bad:?}"
        );
    }
    assert!(matches!(
        service
            .fire_webhook(
                token(&path),
                b"{\"event\": \"tampered\"}",
                Some(&format!("sha256={signature}"))
            )
            .await,
        Err(ApiError::Unauthorized { .. })
    ));
    let run = service
        .fire_webhook(token(&path), body, Some(&format!("sha256={signature}")))
        .await
        .unwrap();
    assert_eq!(run.status, RunState::Queued);
    let listed = service.list_triggers(&PERSON, Some(id)).await.unwrap();
    assert_eq!(listed[0].kind, TriggerKind::Webhook { signed: true });
    assert_eq!(listed[0].last_error, None);
}

#[sqlx::test(migrations = "../store/migrations")]
async fn due_schedules_start_runs_and_move_on(pool: PgPool) {
    let service = service(&pool, Capabilities::LOCAL).await;
    let id = workflow(
        &service,
        &PERSON,
        "workflow \"Nightly\"\n\ninput who: text\n\nstep say uses log.write\n  message is \"Hi {who}\"\n",
    )
    .await;
    let created = service
        .create_trigger(
            &PERSON,
            id,
            "Every minute",
            NewTrigger::Schedule {
                cron: "* * * * *".into(),
            },
            Map::from_iter([("who".to_owned(), json!("Ada"))]),
        )
        .await
        .unwrap();
    let TriggerKind::Schedule { next_run_at, .. } = created.summary.kind else {
        panic!("a schedule");
    };
    let next = next_run_at.unwrap();
    assert!(next - time::OffsetDateTime::now_utc() <= time::Duration::minutes(1));
    assert_eq!(service.fire_due_schedules().await, 0);
    sqlx::query("UPDATE triggers SET next_run_at = now() - interval '1 minute'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(service.fire_due_schedules().await, 1);
    assert_eq!(service.fire_due_schedules().await, 0);
    let runs = service
        .list_runs(&PERSON, Some(id), None, None)
        .await
        .unwrap();
    assert_eq!(runs.items.len(), 1);
    assert_eq!(runs.items[0].trigger, Via::Schedule);
    assert_eq!(
        service
            .get_run(&PERSON, runs.items[0].id)
            .await
            .unwrap()
            .run
            .inputs["who"],
        "Ada"
    );
    let listed = service.list_triggers(&PERSON, Some(id)).await.unwrap();
    let TriggerKind::Schedule { next_run_at, .. } = &listed[0].kind else {
        panic!("a schedule");
    };
    assert!(next_run_at.unwrap() > time::OffsetDateTime::now_utc());
}

#[sqlx::test(migrations = "../store/migrations")]
async fn triggers_are_checked_and_private(pool: PgPool) {
    let service = service(&pool, Capabilities::MANAGED).await;
    let mut people = Vec::new();
    for name in ["ada", "eve"] {
        let session = service
            .sign_in_identity(&parameters_service::Identity {
                kind: parameters_service::ProviderKind::Github,
                id: name.into(),
                verified_email: None,
                name: name.into(),
            })
            .await
            .unwrap();
        people.push(service.session_actor(&session).await.unwrap());
    }
    let (ada, eve) = (people[0], people[1]);
    let id = workflow(&service, &ada, HOOKED).await;
    for (cron, reason) in [
        ("nonsense", "five fields"),
        ("* * * * *", "every 5 minutes"),
        ("0 0 31 2 *", "never runs"),
    ] {
        let refused = service
            .create_trigger(
                &ada,
                id,
                "Often",
                NewTrigger::Schedule { cron: cron.into() },
                Map::new(),
            )
            .await;
        assert!(
            matches!(&refused, Err(ApiError::BadRequest { message }) if message.contains(reason)),
            "{cron}: {refused:?}"
        );
    }
    assert!(matches!(
        service
            .create_trigger(
                &ada,
                id,
                "Bad input",
                NewTrigger::Webhook { signed: false },
                Map::from_iter([("nope".to_owned(), json!(1))])
            )
            .await,
        Err(ApiError::BadRequest { .. })
    ));
    let created = service
        .create_trigger(
            &ada,
            id,
            "Hourly",
            NewTrigger::Schedule {
                cron: "@hourly".into(),
            },
            Map::new(),
        )
        .await
        .unwrap();
    assert!(matches!(
        service
            .create_trigger(
                &eve,
                id,
                "Steal",
                NewTrigger::Webhook { signed: false },
                Map::new()
            )
            .await,
        Err(ApiError::NotFound { .. })
    ));
    assert!(service.list_triggers(&eve, None).await.unwrap().is_empty());
    assert!(matches!(
        service.delete_trigger(&eve, created.summary.id).await,
        Err(ApiError::NotFound { .. })
    ));
    assert!(matches!(
        service
            .set_trigger_enabled(&eve, created.summary.id, false)
            .await,
        Err(ApiError::NotFound { .. })
    ));
}

#[sqlx::test(migrations = "../store/migrations")]
async fn webhooks_still_need_approval_for_privileged_steps(pool: PgPool) {
    let service = service(&pool, Capabilities::LOCAL).await;
    let id = workflow(
        &service,
        &PERSON,
        "workflow \"Shell\"\n\nstep a uses shell.run\n  command is \"echo hi\"\n",
    )
    .await;
    let created = service
        .create_trigger(
            &PERSON,
            id,
            "Hook",
            NewTrigger::Webhook { signed: false },
            Map::new(),
        )
        .await
        .unwrap();
    let path = created.hook_path.unwrap();
    assert!(matches!(
        service.fire_webhook(token(&path), b"{}", None).await,
        Err(ApiError::ApprovalRequired { .. })
    ));
    let review = service.review_workflow(&PERSON, id).await.unwrap();
    service
        .approve_workflow(&PERSON, id, &review.hash)
        .await
        .unwrap();
    assert!(
        service
            .fire_webhook(token(&path), b"{}", None)
            .await
            .is_ok()
    );
}
