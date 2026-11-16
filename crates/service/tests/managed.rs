#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::assert_is_empty,
    reason = "test harness: a failing step should fail the test loudly"
)]

use std::fmt::Write;
use std::sync::Arc;
use std::time::Duration;

use parameters_api::{ApiError, NodeKind, Scope, Scopes, Source, Via};
use parameters_core::{Definition, OrderedMap};
use parameters_service::{
    Actor, Capabilities, EncryptionKey, Identity, Managed, MemoryMailer, Options, ProviderKind,
    Service,
};
use parameters_store::Store;
use serde_json::Map;
use sqlx::PgPool;

async fn managed(pool: &PgPool) -> (Service, MemoryMailer) {
    let mailer = MemoryMailer::default();
    let mut options = Options::new(std::env::temp_dir());
    options.heartbeat = Duration::from_millis(100);
    options.capabilities = Capabilities::MANAGED;
    options.managed = Some(Managed {
        public_url: "https://parameters.test".into(),
        mailer: Some(Arc::new(mailer.clone())),
        providers: Vec::new(),
        trust_proxy: false,
    });
    let service = Service::new(
        Store::from_pool(pool.clone()),
        &EncryptionKey::from_bytes([5; 32]),
        options,
    )
    .await
    .unwrap();
    (service, mailer)
}

fn identity(id: &str, email: Option<&str>) -> Identity {
    Identity {
        kind: ProviderKind::Github,
        id: id.into(),
        verified_email: email.map(str::to_owned),
        name: id.into(),
    }
}

async fn person(service: &Service, id: &str) -> Actor {
    let session = service.sign_in_identity(&identity(id, None)).await.unwrap();
    service.session_actor(&session).await.unwrap()
}

fn text(source: &str) -> Source {
    Source::Text(source.to_owned())
}

fn not_found<T: std::fmt::Debug>(result: &Result<T, ApiError>) {
    assert!(
        matches!(result, Err(ApiError::NotFound { .. })),
        "{result:?}"
    );
}

#[sqlx::test(migrations = "../store/migrations")]
async fn magic_links_sign_in_once_and_are_rate_limited(pool: PgPool) {
    let (service, mailer) = managed(&pool).await;
    service
        .request_email_sign_in(
            "  Ada@Example.COM ",
            "203.0.113.5",
            Some("/connect?code=ABCD-EFGH"),
        )
        .await
        .unwrap();
    let mail = mailer.sent().pop().unwrap();
    assert_eq!(mail.to, "ada@example.com");
    let token = mail
        .text
        .split("token=")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap()
        .to_owned();
    assert!(
        mail.text
            .contains("https://parameters.test/auth/email?token=")
    );
    let signed_in = service.finish_email_sign_in(&token).await.unwrap();
    assert_eq!(signed_in.next, "/connect?code=ABCD-EFGH");
    let session = signed_in.session;
    let actor = service.session_actor(&session).await.unwrap();
    assert_eq!(actor.via, Via::Ui);
    let me = service.me(Some(&actor)).await.unwrap();
    assert_eq!(me.user.unwrap().email.as_deref(), Some("ada@example.com"));
    assert!(matches!(
        service.finish_email_sign_in(&token).await,
        Err(ApiError::Unauthorized { .. })
    ));
    assert!(matches!(
        service.finish_email_sign_in("made-up").await,
        Err(ApiError::Unauthorized { .. })
    ));
    service.sign_out(&session).await.unwrap();
    assert!(service.session_actor(&session).await.is_none());
    assert!(service.session_actor("made-up").await.is_none());
    for _ in 0..4 {
        service
            .request_email_sign_in("ada@example.com", "203.0.113.5", None)
            .await
            .unwrap();
    }
    assert!(matches!(
        service
            .request_email_sign_in("ada@example.com", "203.0.113.6", None)
            .await,
        Err(ApiError::TooManyRequests { .. })
    ));
    assert!(matches!(
        service
            .request_email_sign_in("not an email", "203.0.113.5", None)
            .await,
        Err(ApiError::BadRequest { .. })
    ));
}

#[sqlx::test(migrations = "../store/migrations")]
async fn identities_join_accounts_only_through_verified_emails(pool: PgPool) {
    let (service, mailer) = managed(&pool).await;
    service
        .request_email_sign_in("ada@example.com", "198.51.100.1", None)
        .await
        .unwrap();
    let token = mailer.sent()[0]
        .text
        .split("token=")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap()
        .to_owned();
    let signed_in = service.finish_email_sign_in(&token).await.unwrap();
    assert_eq!(signed_in.next, "/workflows");
    let by_email = service.session_actor(&signed_in.session).await.unwrap();
    let verified = service
        .session_actor(
            &service
                .sign_in_identity(&identity("1", Some("ADA@example.com")))
                .await
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(verified.user, by_email.user);
    let unverified = person(&service, "2").await;
    assert_ne!(unverified.user, by_email.user);
    assert_eq!(person(&service, "2").await.user, unverified.user);
}

#[sqlx::test(migrations = "../store/migrations")]
async fn tokens_carry_only_their_scopes_until_revoked(pool: PgPool) {
    let (service, _) = managed(&pool).await;
    let owner = person(&service, "owner").await;
    let read_only: Scopes = [Scope::Read].into_iter().collect();
    let created = service
        .create_token(&owner, "agent", read_only, Some(30))
        .await
        .unwrap();
    assert!(created.token.starts_with("prm_"));
    assert_eq!(created.token.len(), 4 + 43);
    let agent = service.token_actor(&created.token, Via::Mcp).await.unwrap();
    assert_eq!((agent.user, agent.scopes), (owner.user, Some(read_only)));
    assert!(service.list_workflows(&agent, None, None).await.is_ok());
    assert!(matches!(
        service
            .create_workflow(&agent, &text("workflow \"No\"\n"))
            .await,
        Err(ApiError::Forbidden { .. })
    ));
    assert!(matches!(
        service
            .create_token(&agent, "escalate", Scopes::ALL, None)
            .await,
        Err(ApiError::Forbidden { .. })
    ));
    assert!(matches!(
        service
            .create_token(&owner, "nothing", Scopes::default(), None)
            .await,
        Err(ApiError::BadRequest { .. })
    ));
    assert_eq!(service.tokens(&owner).await.unwrap().len(), 1);
    service
        .revoke_token(&owner, created.summary.id)
        .await
        .unwrap();
    assert!(
        service
            .token_actor(&created.token, Via::Mcp)
            .await
            .is_none()
    );
    assert!(service.token_actor("prm_made-up", Via::Mcp).await.is_none());
    assert!(service.token_actor("not-a-token", Via::Mcp).await.is_none());
}

#[sqlx::test(migrations = "../store/migrations")]
async fn one_account_cannot_reach_anothers_data(pool: PgPool) {
    let (service, _) = managed(&pool).await;
    let ada = person(&service, "ada").await;
    let eve = person(&service, "eve").await;
    let workflow = service
        .create_workflow(
            &ada,
            &text("workflow \"Private\"\n\nstep a uses log.write\n  message is \"hi\"\n"),
        )
        .await
        .unwrap();
    let id = workflow.record.summary.id;
    let hash = workflow.record.hash.clone();
    service.set_secret(&ada, "TOKEN", "hunter2").await.unwrap();
    let definitions: OrderedMap<Definition> = parameters_lang::parse(
        "workflow \"L\"\n\ndefine node ada.say\n  step s uses log.write\n    message is \"x\"\n",
    )
    .workflow
    .definitions;
    service
        .save_node(
            &ada,
            "ada.say",
            definitions.get("ada.say").unwrap().clone(),
            None,
        )
        .await
        .unwrap();
    let run = service.start_run(&ada, id, Map::new()).await.unwrap();

    not_found(&service.get_workflow(&eve, id).await);
    not_found(&service.review_workflow(&eve, id).await);
    not_found(&service.approve_workflow(&eve, id, &hash).await);
    not_found(&service.watch_workflow(&eve, id).await.map(drop));
    not_found(
        &service
            .update_workflow(&eve, id, 1, &text("workflow \"Taken\"\n"))
            .await,
    );
    not_found(&service.delete_workflow(&eve, id).await);
    not_found(&service.start_run(&eve, id, Map::new()).await);
    not_found(&service.get_run(&eve, run.id).await);
    not_found(&service.watch_run(&eve, run.id).await.map(drop));
    not_found(&service.cancel_run(&eve, run.id).await);
    not_found(&service.get_node(&eve, "ada.say").await);
    not_found(&service.delete_secret(&eve, "TOKEN").await);
    assert!(
        service
            .list_workflows(&eve, None, None)
            .await
            .unwrap()
            .items
            .is_empty()
    );
    assert!(
        service
            .list_runs(&eve, None, None, None)
            .await
            .unwrap()
            .items
            .is_empty()
    );
    assert!(service.list_secrets(&eve).await.unwrap().is_empty());
    assert!(service.list_nodes(&eve).await.unwrap().is_empty());
    assert!(
        service
            .node_users(&eve, "ada.say")
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        service
            .node_catalog(&eve)
            .await
            .unwrap()
            .iter()
            .all(|node| node.type_name != "ada.say")
    );
    let eves = service
        .create_workflow(&eve, &text("workflow \"Uses\"\n\nstep a uses ada.say\n"))
        .await;
    assert!(matches!(eves, Err(ApiError::Invalid { .. })), "{eves:?}");

    assert_eq!(
        service
            .get_workflow(&ada, id)
            .await
            .unwrap()
            .summary
            .version,
        1
    );
    assert_eq!(service.list_secrets(&ada).await.unwrap().len(), 1);
}

#[sqlx::test(migrations = "../store/migrations")]
async fn the_managed_sandbox_and_quotas_apply(pool: PgPool) {
    let (service, _) = managed(&pool).await;
    let ada = person(&service, "ada").await;
    let catalog = service.node_catalog(&ada).await.unwrap();
    assert!(catalog.iter().all(|node| !node.privileged), "{catalog:?}");
    let shell = service
        .create_workflow(
            &ada,
            &text("workflow \"Shell\"\n\nstep a uses shell.run\n  command is \"id\"\n"),
        )
        .await
        .unwrap();
    let refused = service
        .start_run(&ada, shell.record.summary.id, Map::new())
        .await;
    assert!(
        matches!(&refused, Err(ApiError::Forbidden { message }) if message.contains("shell.run")),
        "{refused:?}"
    );
    let mut big = String::from("workflow \"Big\"\n");
    for index in 0..501 {
        let _ = write!(big, "\nstep s{index} uses log.write\n  message is \"x\"\n");
    }
    let big = service.create_workflow(&ada, &text(&big)).await.unwrap();
    assert!(matches!(
        service
            .start_run(&ada, big.record.summary.id, Map::new())
            .await,
        Err(ApiError::Forbidden { .. })
    ));
    let small = service
        .create_workflow(
            &ada,
            &text("workflow \"Small\"\n\nstep a uses log.write\n  message is \"x\"\n"),
        )
        .await
        .unwrap();
    for _ in 0..5 {
        service
            .start_run(&ada, small.record.summary.id, Map::new())
            .await
            .unwrap();
    }
    assert!(matches!(
        service
            .start_run(&ada, small.record.summary.id, Map::new())
            .await,
        Err(ApiError::TooManyRequests { .. })
    ));
}

#[sqlx::test(migrations = "../store/migrations")]
async fn devices_connect_once_a_person_approves_their_code(pool: PgPool) {
    let (service, _) = managed(&pool).await;
    let ada = person(&service, "ada").await;
    let start = service.start_device("Laptop", "192.0.2.1").await.unwrap();
    assert_eq!(start.user_code.len(), 9);
    assert!(
        start
            .verification_uri
            .starts_with("https://parameters.test/connect?code=")
    );
    assert_eq!(
        service.claim_device(&start.device_code).await.unwrap(),
        None
    );
    let token = service
        .create_token(&ada, "agent", Scopes::ALL, None)
        .await
        .unwrap()
        .token;
    let agent = service.token_actor(&token, Via::Api).await.unwrap();
    assert!(matches!(
        service.approve_device(&agent, &start.user_code).await,
        Err(ApiError::Forbidden { .. })
    ));
    assert!(matches!(
        service.approve_device(&ada, "ZZZZ-ZZZZ").await,
        Err(ApiError::NotFound { .. })
    ));
    let typed = start.user_code.to_lowercase().replace('-', " ");
    assert_eq!(
        service.approve_device(&ada, &typed).await.unwrap(),
        "Laptop"
    );
    let session = service
        .claim_device(&start.device_code)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        service.session_actor(&session).await.unwrap().user,
        ada.user
    );
    assert!(matches!(
        service.claim_device(&start.device_code).await,
        Err(ApiError::Unauthorized { .. })
    ));
    assert!(matches!(
        service.claim_device("made-up").await,
        Err(ApiError::Unauthorized { .. })
    ));
    for next in [
        "https://evil.example",
        "//evil.example",
        "/\\evil",
        "relative",
    ] {
        assert_eq!(parameters_service::safe_next(Some(next)), None, "{next}");
    }
    assert_eq!(
        parameters_service::safe_next(Some("/runs")).as_deref(),
        Some("/runs")
    );
}

#[sqlx::test(migrations = "../store/migrations")]
async fn signed_out_visitors_see_only_the_public_docs(pool: PgPool) {
    let (service, _) = managed(&pool).await;
    let nodes = service.documented_nodes(None).await.unwrap();
    assert!(nodes.iter().any(|node| node.type_name == "http.request"));
    assert!(
        nodes
            .iter()
            .all(|node| node.kind == NodeKind::Builtin && !node.privileged)
    );
    let limits = service.limits();
    assert!(!limits.privileged_nodes && !limits.private_network);
    assert_eq!(limits.max_steps, Some(500));
    assert_eq!(limits.min_schedule_minutes, Some(5));
}
