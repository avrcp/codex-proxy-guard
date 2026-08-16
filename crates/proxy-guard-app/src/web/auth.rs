use std::sync::Arc;

use axum::{
    extract::{Request, State},
    http::{HeaderMap, HeaderName, HeaderValue, Method, header},
    middleware::Next,
    response::Response,
};

use super::response::unauthorized;
use super::state::ManagerState;

/// Reject any request that is not loopback-scoped and does not carry the per-session
/// capability token. Mutations must also come from the Manager's own origin.
pub async fn require_auth(
    State(state): State<Arc<ManagerState>>,
    method: Method,
    headers: HeaderMap,
    request: Request,
    next: Next,
) -> Response {
    let expected_host = format!("127.0.0.1:{}", state.port);
    let host_ok = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|host| host == expected_host);
    let token_ok = headers
        .get("x-codex-guard-manager")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|token| token == state.token);
    if !host_ok || !token_ok {
        return unauthorized();
    }
    if !matches!(method, Method::GET) {
        let expected_origin = format!("http://127.0.0.1:{}", state.port);
        let origin_ok = headers
            .get(header::ORIGIN)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|origin| origin == expected_origin);
        if !origin_ok {
            return unauthorized();
        }
    }
    *state.last_activity.lock().await = std::time::Instant::now();
    next.run(request).await
}

/// Apply the always-on hardening headers to every response, including asset and
/// error responses.
pub async fn security_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        HeaderName::from_static("cross-origin-resource-policy"),
        HeaderValue::from_static("same-origin"),
    );
    response
}
