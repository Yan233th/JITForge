use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
};

use axum::{
    Json,
    body::Body,
    extract::State,
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Duration, Utc};
use jit_protocol::{SessionLoginRequest, SessionResponse};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{ApiError, AppState, constant_time_eq};

pub const CSRF_HEADER: &str = "x-jitforge-csrf";
const SESSION_COOKIE: &str = "jitforge_session";
const SESSION_TTL_HOURS: i64 = 12;
const MAX_SESSIONS: usize = 1024;

const INDEX_HTML: &str = include_str!("../../../web/console/index.html");
const APP_CSS: &str = include_str!("../../../web/console/app.css");
const APP_JS: &str = include_str!("../../../web/console/app.js");

#[derive(Clone, Debug)]
pub struct WebSession {
    pub id: String,
    pub csrf_token: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Clone, Default)]
pub struct SessionStore {
    inner: Arc<Mutex<HashMap<String, WebSession>>>,
}

impl SessionStore {
    pub fn create(&self) -> WebSession {
        self.create_for(Duration::hours(SESSION_TTL_HOURS))
    }

    fn create_for(&self, ttl: Duration) -> WebSession {
        let now = Utc::now();
        let mut sessions = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        sessions.retain(|_, session| session.expires_at > now);
        if sessions.len() >= MAX_SESSIONS
            && let Some(oldest) = sessions
                .values()
                .min_by_key(|session| session.expires_at)
                .map(|session| session.id.clone())
        {
            sessions.remove(&oldest);
        }
        let session = WebSession {
            id: Uuid::new_v4().to_string(),
            csrf_token: Uuid::new_v4().to_string(),
            expires_at: now + ttl,
        };
        sessions.insert(session.id.clone(), session.clone());
        session
    }

    pub fn get(&self, id: &str) -> Option<WebSession> {
        let now = Utc::now();
        let mut sessions = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        sessions.retain(|_, session| session.expires_at > now);
        sessions.get(id).cloned()
    }

    pub fn remove(&self, id: &str) {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(id);
    }
}

pub async fn root() -> Redirect {
    Redirect::temporary("/ui/")
}

pub async fn index() -> Response {
    static_response("text/html; charset=utf-8", versioned_index())
}

pub async fn css() -> Response {
    static_response("text/css; charset=utf-8", APP_CSS)
}

pub async fn js() -> Response {
    static_response("text/javascript; charset=utf-8", APP_JS)
}

fn versioned_index() -> &'static str {
    static VERSIONED_INDEX: OnceLock<String> = OnceLock::new();
    VERSIONED_INDEX.get_or_init(|| {
        let mut hasher = Sha256::new();
        hasher.update(APP_CSS.as_bytes());
        hasher.update([0]);
        hasher.update(APP_JS.as_bytes());
        let version = URL_SAFE_NO_PAD.encode(hasher.finalize());
        INDEX_HTML
            .replace("/ui/app.css", &format!("/ui/app.css?v={version}"))
            .replace("/ui/app.js", &format!("/ui/app.js?v={version}"))
    })
}

pub async fn login(
    State(state): State<AppState>,
    Json(request): Json<SessionLoginRequest>,
) -> Result<Response, ApiError> {
    if request.token.is_empty()
        || request.token.len() > 4096
        || !constant_time_eq(request.token.as_bytes(), state.auth_token.as_bytes())
    {
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "invalid authentication token",
        ));
    }
    let session = state.sessions.create();
    let mut response = Json(session_response(&session)).into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&format!(
            "{SESSION_COOKIE}={}; HttpOnly; SameSite=Strict; Path=/v1; Max-Age={}",
            session.id,
            SESSION_TTL_HOURS * 60 * 60
        ))
        .map_err(|_| ApiError::internal("failed to create session cookie"))?,
    );
    Ok(response)
}

pub async fn current_session(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<SessionResponse>, ApiError> {
    let session = session_from_headers(&state.sessions, &headers).ok_or_else(unauthorized)?;
    Ok(Json(session_response(&session)))
}

pub async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let session = session_from_headers(&state.sessions, &headers).ok_or_else(unauthorized)?;
    if !csrf_matches(&session, &headers) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "csrf_failed",
            "a valid CSRF token is required",
        ));
    }
    state.sessions.remove(&session.id);
    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_static(
            "jitforge_session=; HttpOnly; SameSite=Strict; Path=/v1; Max-Age=0",
        ),
    );
    Ok(response)
}

pub fn session_from_headers(store: &SessionStore, headers: &HeaderMap) -> Option<WebSession> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    let id = raw.split(';').find_map(|part| {
        let (name, value) = part.trim().split_once('=')?;
        (name == SESSION_COOKIE).then_some(value)
    })?;
    store.get(id)
}

pub fn csrf_matches(session: &WebSession, headers: &HeaderMap) -> bool {
    headers
        .get(CSRF_HEADER)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| constant_time_eq(value.as_bytes(), session.csrf_token.as_bytes()))
}

fn session_response(session: &WebSession) -> SessionResponse {
    SessionResponse {
        csrf_token: session.csrf_token.clone(),
        expires_at: session.expires_at.to_rfc3339(),
    }
}

fn unauthorized() -> ApiError {
    ApiError::new(
        StatusCode::UNAUTHORIZED,
        "unauthorized",
        "a valid browser session is required",
    )
}

fn static_response(content_type: &'static str, content: &'static str) -> Response {
    let mut response = Response::new(Body::from(content));
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    headers.insert(
        HeaderName::from_static("content-security-policy"),
        HeaderValue::from_static(
            "default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self'; object-src 'none'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'",
        ),
    );
    headers.insert(
        HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        HeaderName::from_static("referrer-policy"),
        HeaderValue::from_static("no-referrer"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_expire_and_are_removed() {
        let store = SessionStore::default();
        let session = store.create_for(Duration::seconds(-1));
        assert!(store.get(&session.id).is_none());
    }

    #[test]
    fn parses_only_the_named_session_cookie() {
        let store = SessionStore::default();
        let session = store.create();
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("other=x; {SESSION_COOKIE}={}", session.id)).unwrap(),
        );
        assert_eq!(
            session_from_headers(&store, &headers).unwrap().id,
            session.id
        );
    }

    #[test]
    fn csrf_requires_the_exact_session_token() {
        let store = SessionStore::default();
        let session = store.create();
        let mut headers = HeaderMap::new();
        headers.insert(CSRF_HEADER, HeaderValue::from_static("wrong"));
        assert!(!csrf_matches(&session, &headers));
        headers.insert(
            CSRF_HEADER,
            HeaderValue::from_str(&session.csrf_token).unwrap(),
        );
        assert!(csrf_matches(&session, &headers));
    }

    #[test]
    fn static_assets_apply_browser_security_headers() {
        let response = static_response("text/plain", "test");
        assert!(response.headers().contains_key("content-security-policy"));
        assert_eq!(response.headers()["x-content-type-options"], "nosniff");
        assert_eq!(response.headers()["referrer-policy"], "no-referrer");
    }

    #[test]
    fn index_uses_content_versioned_asset_urls() {
        let index = versioned_index();
        assert!(index.contains("/ui/app.css?v="));
        assert!(index.contains("/ui/app.js?v="));
        assert!(!index.contains("href=\"/ui/app.css\""));
        assert!(!index.contains("src=\"/ui/app.js\""));
    }
}
