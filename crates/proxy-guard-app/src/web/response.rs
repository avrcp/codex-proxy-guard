use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use proxy_guard_core::redact_text;
use proxy_guard_network::NetworkError;

use crate::web::dto::ErrorDto;

/// Web-layer error mapped to a stable HTTP status and a redacted message.
#[derive(Debug)]
pub enum AppError {
    BadRequest(String),
    NotFound(String),
    Conflict(String),
    OperationBusy,
    Unprocessable(String),
    Internal(String),
}

impl AppError {
    fn code(&self) -> &'static str {
        match self {
            Self::BadRequest(_) => "BAD_REQUEST",
            Self::NotFound(_) => "NOT_FOUND",
            Self::Conflict(_) => "CONFLICT",
            Self::OperationBusy => "OPERATION_BUSY",
            Self::Unprocessable(_) => "UNPROCESSABLE",
            Self::Internal(_) => "INTERNAL_ERROR",
        }
    }

    fn status(&self) -> StatusCode {
        match self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::OperationBusy => StatusCode::CONFLICT,
            Self::Unprocessable(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn message(&self) -> String {
        match self {
            Self::BadRequest(message)
            | Self::NotFound(message)
            | Self::Conflict(message)
            | Self::Unprocessable(message)
            | Self::Internal(message) => redact_text(message),
            Self::OperationBusy => {
                "Another operation is already running; wait for it to finish".into()
            }
        }
    }
}

impl From<NetworkError> for AppError {
    fn from(error: NetworkError) -> Self {
        match error {
            NetworkError::NotFound => Self::NotFound("resource was not found".into()),
            NetworkError::SubscriptionUrl(message) | NetworkError::Parse(message) => {
                Self::Unprocessable(message)
            }
            NetworkError::Node(message) if message.contains("not found") => Self::NotFound(message),
            NetworkError::Benchmark(message) if message.contains("no active") => {
                Self::Conflict(message)
            }
            other => Self::Internal(other.to_string()),
        }
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let body = Json(ErrorDto {
            code: self.code().into(),
            message: self.message(),
        });
        (self.status(), body).into_response()
    }
}

pub fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(ErrorDto {
            code: "UNAUTHORIZED".into(),
            message: "invalid manager token".into(),
        }),
    )
        .into_response()
}
