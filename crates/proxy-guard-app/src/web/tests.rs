use std::{path::PathBuf, sync::Arc};

use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use proxy_guard_core::{GuardConfig, ProxyMode, SubscriptionSource};
use proxy_guard_network::StoredSubscription;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

use super::{dto, routes::build_router, server::ManagerServer, state::ManagerState};

const TOKEN: &str = "test-session-token";
const HOST: &str = "127.0.0.1:43210";
const ACTIVE_ID: &str = "11111111-1111-1111-1111-111111111111";

fn test_state() -> Arc<ManagerState> {
    test_state_with(GuardConfig::default())
}

fn test_state_with(config: GuardConfig) -> Arc<ManagerState> {
    let (tx, _rx) = mpsc::channel(8);
    Arc::new(ManagerState::new(
        tx,
        config,
        PathBuf::from("config.toml"),
        TOKEN.into(),
        43210,
        CancellationToken::new(),
    ))
}

fn request(method: Method, uri: &str, headers: &[(&str, &str)]) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    builder.body(Body::empty()).unwrap()
}

#[tokio::test]
async fn api_rejects_missing_and_wrong_token() {
    let app = build_router(test_state());

    let missing = app
        .clone()
        .oneshot(request(
            Method::GET,
            "/api/v1/state",
            &[(header::HOST.as_str(), HOST)],
        ))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);

    let wrong = app
        .oneshot(request(
            Method::GET,
            "/api/v1/state",
            &[
                (header::HOST.as_str(), HOST),
                ("x-codex-guard-manager", "wrong-token"),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_accepts_valid_token_and_host() {
    let app = build_router(test_state());
    let response = app
        .oneshot(request(
            Method::GET,
            "/api/v1/state",
            &[
                (header::HOST.as_str(), HOST),
                ("x-codex-guard-manager", TOKEN),
            ],
        ))
        .await
        .unwrap();
    assert_ne!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_rejects_wrong_host() {
    let app = build_router(test_state());
    let response = app
        .oneshot(request(
            Method::GET,
            "/api/v1/state",
            &[
                (header::HOST.as_str(), "127.0.0.1:9999"),
                ("x-codex-guard-manager", TOKEN),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_rejects_foreign_origin_mutation() {
    let app = build_router(test_state());
    let response = app
        .oneshot(request(
            Method::POST,
            "/api/v1/manager/close",
            &[
                (header::HOST.as_str(), HOST),
                (header::ORIGIN.as_str(), "http://evil.example"),
                ("x-codex-guard-manager", TOKEN),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_accepts_same_origin_mutation() {
    let app = build_router(test_state());
    let response = app
        .oneshot(request(
            Method::POST,
            "/api/v1/manager/close",
            &[
                (header::HOST.as_str(), HOST),
                (header::ORIGIN.as_str(), "http://127.0.0.1:43210"),
                ("x-codex-guard-manager", TOKEN),
            ],
        ))
        .await
        .unwrap();
    assert_ne!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn static_assets_are_served_with_security_headers() {
    let app = build_router(test_state());
    let response = app
        .clone()
        .oneshot(request(Method::GET, "/", &[]))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response
            .headers()
            .get("content-security-policy")
            .is_some_and(|value| value.to_str().unwrap().contains("script-src 'self'"))
    );
    assert_eq!(response.headers().get("cache-control").unwrap(), "no-store");

    let js = app
        .clone()
        .oneshot(request(Method::GET, "/app.js", &[]))
        .await
        .unwrap();
    assert_eq!(js.status(), StatusCode::OK);
    let css = app
        .oneshot(request(Method::GET, "/style.css", &[]))
        .await
        .unwrap();
    assert_eq!(css.status(), StatusCode::OK);
}

#[tokio::test]
async fn api_responses_carry_hardening_headers() {
    let app = build_router(test_state());
    let response = app
        .oneshot(request(
            Method::GET,
            "/api/v1/state",
            &[
                (header::HOST.as_str(), HOST),
                ("x-codex-guard-manager", TOKEN),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(response.headers().get("cache-control").unwrap(), "no-store");
    assert_eq!(
        response.headers().get("x-content-type-options").unwrap(),
        "nosniff"
    );
    assert_eq!(
        response.headers().get("referrer-policy").unwrap(),
        "no-referrer"
    );
}

#[tokio::test]
async fn manager_server_binds_loopback_and_stops_on_shutdown() {
    let (tx, _rx) = mpsc::channel(8);
    let parent = CancellationToken::new();
    let server = ManagerServer::start(
        GuardConfig::default(),
        PathBuf::from("config.toml"),
        tx,
        &parent,
    )
    .await
    .expect("start manager server");
    assert!(server.display_url.starts_with("http://127.0.0.1:"));
    server.shutdown.cancel();
    let _ = server.task.await;
    assert!(!parent.is_cancelled());
}

#[tokio::test]
async fn deleting_the_active_subscription_is_rejected() {
    let mut config = GuardConfig::default();
    config.proxy.mode = ProxyMode::Managed;
    config.managed.subscription_id = ACTIVE_ID.into();
    let app = build_router(test_state_with(config));
    let response = app
        .oneshot(request(
            Method::DELETE,
            &format!("/api/v1/subscriptions/{ACTIVE_ID}"),
            &[
                (header::HOST.as_str(), HOST),
                (header::ORIGIN.as_str(), "http://127.0.0.1:43210"),
                ("x-codex-guard-manager", TOKEN),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[test]
fn subscription_dto_never_serializes_a_url() {
    let stored = StoredSubscription {
        source: SubscriptionSource::new("Airport").expect("source"),
        bindings: Vec::new(),
        subscription_dir: PathBuf::from("unused"),
    };
    let value = serde_json::to_value(dto::subscription_dto(&stored, None)).expect("serialize");
    assert!(value.get("url").is_none());
    assert!(value.get("token").is_none());
    assert_eq!(value["name"], "Airport");
    assert_eq!(value["active"], false);
}
