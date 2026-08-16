use axum::{
    http::{HeaderValue, StatusCode, header},
    response::Response,
};

pub const INDEX_HTML: &str = include_str!("../../assets/manager/index.html");
pub const APP_JS: &str = include_str!("../../assets/manager/app.js");
pub const STYLE_CSS: &str = include_str!("../../assets/manager/style.css");

const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self'; \
connect-src 'self'; img-src 'self' data:; object-src 'none'; base-uri 'none'; \
frame-ancestors 'none'; form-action 'self'";

fn asset_response(content: &str, content_type: &'static str, csp: bool) -> Response {
    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type);
    if csp {
        builder = builder.header(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(CSP),
        );
    }
    builder
        .body(axum::body::Body::from(content.to_owned()))
        .unwrap()
}

pub async fn index() -> Response {
    asset_response(INDEX_HTML, "text/html; charset=utf-8", true)
}

pub async fn app_js() -> Response {
    asset_response(APP_JS, "text/javascript; charset=utf-8", false)
}

pub async fn style_css() -> Response {
    asset_response(STYLE_CSS, "text/css; charset=utf-8", false)
}
