//! Thin typed wrapper over the server's HTTP API

use std::time::Duration;

use proto::{
    Ack, ApiError, ChatRequest, CreateRoomResponse, ErrorCode, Health, JoinByCodeRequest,
    JoinResponse, MoveRequest, RegisterRequest, RegisterResponse, RoomSnapshot, UpdateMeRequest,
    UserInfo,
};
use serde::de::DeserializeOwned;

pub const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("{}", .0.message)]
    Api(ApiError),
    #[error("this client ({CLIENT_VERSION}) is too old; the server requires at least {min}")]
    Outdated { min: String },
    #[error("network error: {0}")]
    Network(String),
    #[error("unexpected response: {0}")]
    Decode(String),
}

impl ClientError {
    pub fn code(&self) -> Option<ErrorCode> {
        match self {
            Self::Api(e) => Some(e.code),
            _ => None,
        }
    }

    /// Not retryable (auth / membership / outdated)
    pub fn is_fatal_for_stream(&self) -> bool {
        matches!(self, Self::Outdated { .. })
            || matches!(
                self.code(),
                Some(ErrorCode::Unauthorized | ErrorCode::NotFound | ErrorCode::NotAMember)
            )
    }
}

impl From<reqwest::Error> for ClientError {
    fn from(e: reqwest::Error) -> Self {
        if e.is_decode() {
            Self::Decode(e.to_string())
        } else {
            Self::Network(e.to_string())
        }
    }
}

pub type ApiResult<T> = Result<T, ClientError>;

#[derive(Clone, Debug)]
pub struct Api {
    http: reqwest::Client,
    base: String,
    token: Option<String>,
}

impl Api {
    pub fn new(base: impl Into<String>, token: Option<String>) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .user_agent(format!("gomoku/{CLIENT_VERSION}"))
            .build()
            .expect("reqwest client");
        Self {
            http,
            base: base.into(),
            token,
        }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.token = Some(token.into());
        self
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    fn decorate(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let rb = rb.header(proto::CLIENT_VERSION_HEADER, CLIENT_VERSION);
        match &self.token {
            Some(t) => rb.bearer_auth(t),
            None => rb,
        }
    }

    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        self.decorate(self.http.get(self.url(path)))
            .timeout(Duration::from_secs(20))
    }

    fn post(&self, path: &str) -> reqwest::RequestBuilder {
        self.decorate(self.http.post(self.url(path)))
            .timeout(Duration::from_secs(20))
    }

    fn patch(&self, path: &str) -> reqwest::RequestBuilder {
        self.decorate(self.http.patch(self.url(path)))
            .timeout(Duration::from_secs(20))
    }

    async fn send<T: DeserializeOwned>(rb: reqwest::RequestBuilder) -> ApiResult<T> {
        let resp = rb.send().await?;
        Self::handle(resp).await
    }

    /// Also used by the SSE connector
    pub async fn handle<T: DeserializeOwned>(resp: reqwest::Response) -> ApiResult<T> {
        let status = resp.status();
        if status.is_success() {
            return Ok(resp.json::<T>().await?);
        }
        Err(Self::error_from(resp).await)
    }

    pub async fn error_from(resp: reqwest::Response) -> ClientError {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        match serde_json::from_str::<ApiError>(&text) {
            Ok(err) if err.code == ErrorCode::ClientOutdated => ClientError::Outdated {
                min: err.min_client_version.unwrap_or_else(|| "unknown".into()),
            },
            Ok(err) => ClientError::Api(err),
            Err(_) if status.as_u16() == 426 => ClientError::Outdated {
                min: "unknown".into(),
            },
            Err(_) => ClientError::Decode(format!("HTTP {status}: {}", text.trim())),
        }
    }

    pub async fn health(&self) -> ApiResult<Health> {
        Self::send(self.get("/healthz")).await
    }

    pub async fn register(&self, display_name: Option<String>) -> ApiResult<RegisterResponse> {
        let body = RegisterRequest { display_name };
        Self::send(self.post("/users/anonymous").json(&body)).await
    }

    pub async fn me(&self) -> ApiResult<UserInfo> {
        Self::send(self.get("/users/me")).await
    }

    pub async fn update_me(&self, display_name: &str) -> ApiResult<UserInfo> {
        let body = UpdateMeRequest {
            display_name: display_name.to_owned(),
        };
        Self::send(self.patch("/users/me").json(&body)).await
    }

    pub async fn create_room(&self) -> ApiResult<CreateRoomResponse> {
        Self::send(self.post("/rooms")).await
    }

    pub async fn join_by_code(&self, invite_code: &str) -> ApiResult<JoinResponse> {
        let body = JoinByCodeRequest {
            invite_code: invite_code.to_owned(),
        };
        Self::send(self.post("/rooms/join").json(&body)).await
    }

    pub async fn join_room(&self, room_id: &str) -> ApiResult<JoinResponse> {
        Self::send(self.post(&format!("/rooms/{room_id}/join"))).await
    }

    pub async fn snapshot(&self, room_id: &str) -> ApiResult<RoomSnapshot> {
        Self::send(self.get(&format!("/rooms/{room_id}"))).await
    }

    pub async fn play(&self, room_id: &str, req: MoveRequest) -> ApiResult<Ack> {
        Self::send(self.post(&format!("/rooms/{room_id}/move")).json(&req)).await
    }

    pub async fn chat(&self, room_id: &str, text: &str) -> ApiResult<Ack> {
        let body = ChatRequest {
            text: text.to_owned(),
        };
        Self::send(self.post(&format!("/rooms/{room_id}/chat")).json(&body)).await
    }

    pub async fn resign(&self, room_id: &str) -> ApiResult<Ack> {
        Self::send(self.post(&format!("/rooms/{room_id}/resign"))).await
    }

    pub async fn ready(&self, room_id: &str) -> ApiResult<Ack> {
        Self::send(self.post(&format!("/rooms/{room_id}/ready"))).await
    }

    /// No overall timeout: long-lived stream
    pub fn events_request(
        &self,
        room_id: &str,
        last_event_id: Option<u64>,
    ) -> reqwest::RequestBuilder {
        let rb = self
            .decorate(self.http.get(self.url(&format!("/rooms/{room_id}/events"))))
            .header("Accept", "text/event-stream");
        match last_event_id {
            Some(id) => rb.header("Last-Event-ID", id.to_string()),
            None => rb,
        }
    }
}
