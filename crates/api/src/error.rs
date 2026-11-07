use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

#[derive(Debug)]
pub enum ApiError {
    Http { status: StatusCode, message: Value },
    Raw { status: StatusCode, body: Value },
    Internal(anyhow::Error),
}

pub type ApiResult<T> = Result<T, ApiError>;

fn reason(status: StatusCode) -> &'static str {
    status.canonical_reason().unwrap_or("Error")
}

impl ApiError {
    pub fn status(status: StatusCode, message: impl Into<Value>) -> ApiError {
        ApiError::Http {
            status,
            message: message.into(),
        }
    }

    pub fn bad_request(message: impl Into<Value>) -> ApiError {
        ApiError::status(StatusCode::BAD_REQUEST, message)
    }

    pub fn not_found(message: impl Into<Value>) -> ApiError {
        ApiError::status(StatusCode::NOT_FOUND, message)
    }

    pub fn forbidden(message: impl Into<Value>) -> ApiError {
        ApiError::status(StatusCode::FORBIDDEN, message)
    }

    pub fn unauthorized(message: impl Into<Value>) -> ApiError {
        ApiError::status(StatusCode::UNAUTHORIZED, message)
    }

    pub fn conflict(message: impl Into<Value>) -> ApiError {
        ApiError::status(StatusCode::CONFLICT, message)
    }

    pub fn guard() -> ApiError {
        ApiError::forbidden("Forbidden resource")
    }

    pub fn bare(status: StatusCode) -> ApiError {
        ApiError::Raw {
            status,
            body: json!({ "statusCode": status.as_u16(), "message": reason(status) }),
        }
    }

    pub fn raw(status: StatusCode, body: Value) -> ApiError {
        ApiError::Raw { status, body }
    }

    pub fn throttled() -> ApiError {
        ApiError::Raw {
            status: StatusCode::TOO_MANY_REQUESTS,
            body: json!({ "statusCode": 429, "message": "ThrottlerException: Too Many Requests" }),
        }
    }

    pub fn timeout(ms: u128) -> ApiError {
        ApiError::status(StatusCode::GATEWAY_TIMEOUT, format!("Request timed out after {ms}ms."))
    }

    pub fn route_not_found(method: &str, path: &str) -> ApiError {
        ApiError::Raw {
            status: StatusCode::NOT_FOUND,
            body: json!({ "message": format!("Cannot {method} {path}"), "error": "Not Found", "statusCode": 404 }),
        }
    }
}

impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(e: E) -> ApiError {
        ApiError::Internal(e.into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            ApiError::Http { status, message } => {
                let body = json!({ "statusCode": status.as_u16(), "message": message, "error": reason(status) });
                (status, axum::Json(body)).into_response()
            }
            ApiError::Raw { status, body } => (status, axum::Json(body)).into_response(),
            ApiError::Internal(e) => {
                tracing::error!("{e:#}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    axum::Json(json!({ "statusCode": 500, "message": "Internal server error" })),
                )
                    .into_response()
            }
        }
    }
}
