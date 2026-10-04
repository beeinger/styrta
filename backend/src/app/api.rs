use std::convert::Infallible;
use std::sync::Arc;
use std::time::Instant;

use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Extension, FromRequest, Json, Path, Query, Request, State};
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use bytes::Bytes;
use chrono::Utc;
use futures::StreamExt;
use serde::Serialize;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tower_http::trace::TraceLayer;
use utoipa::{Modify, OpenApi, ToSchema};
use utoipa_swagger_ui::SwaggerUi;
use uuid::Uuid;

use crate::appdb::{self, Store};
use crate::auth::{self, Claims};
use crate::config::Config;
use crate::embed::Embedder;
use crate::harness::{self, Services};
use crate::llm::{self, Model};
use crate::rank::{self, TimeWindow};
use crate::speech::Speech;
use crate::sse::{self, Kind};
use crate::tools::{self, ToolName};

const JSON_LIMIT: usize = 64 * 1024;
const AUDIO_LIMIT: usize = 20 * 1024 * 1024;
const HISTORY_LIMIT: i64 = 3;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub store: Store,
    pub model: Arc<dyn Model>,
    pub speech: Arc<dyn Speech>,
    pub embedder: Arc<dyn Embedder>,
    pub geocoder: Arc<dyn crate::places::Geocoder>,
}

/// New account. `locale` defaults to `pl`. `en` selects English replies.
#[derive(Clone, Debug, serde::Deserialize, ToSchema)]
pub struct CreateUser {
    pub display_name: String,
    pub locale: Option<String>,
}

/// Access and refresh tokens for a new account.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct Session {
    pub id: Uuid,
    pub display_name: String,
    pub access_token: String,
    pub refresh_token: String,
    pub access_expires_at: chrono::DateTime<Utc>,
    pub refresh_expires_at: chrono::DateTime<Utc>,
}

#[derive(Clone, Debug, serde::Deserialize, ToSchema)]
pub struct Refresh {
    pub refresh_token: String,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct Refreshed {
    pub access_token: String,
    pub refresh_token: String,
    pub access_expires_at: chrono::DateTime<Utc>,
    pub refresh_expires_at: chrono::DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct Me {
    pub id: Uuid,
    pub display_name: String,
    pub locale: String,
    pub age_band: Option<String>,
    pub gender: Option<String>,
    pub mobility: Option<String>,
    pub sportiness: Option<i16>,
    pub women_only: bool,
    pub time_window: Option<TimeWindow>,
    pub likes: Vec<String>,
    pub dislikes: Vec<String>,
}

#[derive(Clone, Debug, serde::Deserialize, ToSchema, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct NearbyQuery {
    pub lat: f64,
    pub lng: f64,
    pub zoom: f64,
    pub bbox: Option<String>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct NearbyEvent {
    pub id: Uuid,
    pub title: String,
    pub emoji: String,
    pub signed_count: i64,
    pub capacity: Option<i32>,
    pub place_name: String,
    pub latitude: f64,
    pub longitude: f64,
    pub starts_at: chrono::DateTime<Utc>,
    pub host_name: String,
}

/// `emoji` omitted: the server accepts the turn and chooses one grapheme.
/// `emoji` set: the event is created in this request.
#[derive(Clone, Debug, serde::Deserialize, ToSchema)]
pub struct CreateEvent {
    pub title: String,
    pub emoji: Option<String>,
    pub description: Option<String>,
    pub starts_at: chrono::DateTime<Utc>,
    pub place_name: String,
    pub kind: String,
    pub lat: f64,
    pub lon: f64,
    pub capacity: Option<i32>,
    #[serde(default)]
    pub activity_tags: Vec<String>,
}

#[derive(Clone, Debug, serde::Deserialize, ToSchema, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct MessageQuery {
    pub before: Option<Uuid>,
    pub limit: Option<i64>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct HistoryMessage {
    pub id: Uuid,
    pub role: String,
    pub body: String,
    pub created_at: chrono::DateTime<Utc>,
}

/// JSON body for a text turn. Audio turns are `multipart/form-data` with a file field named `audio`.
#[derive(Clone, Debug, serde::Deserialize, ToSchema)]
pub struct PostMessage {
    pub text: Option<String>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct AcceptedTurn {
    pub turn_id: Uuid,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct TurnView {
    pub status: String,
    pub user_text: Option<String>,
    pub reply_text: Option<String>,
    pub audio_ready: bool,
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    message: &'static str,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorBody {
                error: self.message.to_string(),
            }),
        )
            .into_response()
    }
}

#[derive(Serialize, ToSchema)]
struct ErrorBody {
    error: String,
}

fn bad(message: &'static str) -> ApiError {
    ApiError {
        status: StatusCode::BAD_REQUEST,
        message,
    }
}

fn unauthorized() -> ApiError {
    ApiError {
        status: StatusCode::UNAUTHORIZED,
        message: "unauthorized",
    }
}

fn not_found() -> ApiError {
    ApiError {
        status: StatusCode::NOT_FOUND,
        message: "not found",
    }
}

fn conflict() -> ApiError {
    ApiError {
        status: StatusCode::CONFLICT,
        message: "event is full",
    }
}

fn internal() -> ApiError {
    ApiError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        message: "internal error",
    }
}

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/v1/users", post(create_user))
        .route("/v1/auth/refresh", post(refresh_session))
        .route("/v1/stream", get(user_stream))
        .route("/v1/me", get(get_me).patch(patch_me))
        .route("/v1/events/nearby", get(nearby))
        .route("/v1/events/{id}", get(get_event))
        .route("/v1/events", post(create_event))
        .route("/v1/events/{id}/join", post(join_event))
        .route("/v1/events/{id}/cancel", post(cancel_event))
        .route("/v1/events/{id}/complete", post(complete_event))
        .route("/v1/me/events", get(my_events))
        .route("/v1/chat/messages", get(list_messages).post(post_message))
        .route("/v1/chat/turns/{id}", get(get_turn))
        .route("/v1/chat/turns/{id}/audio", get(get_audio))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_access,
        ))
        .layer(DefaultBodyLimit::max(AUDIO_LIMIT))
        .layer(TraceLayer::new_for_http())
        .with_state(state);
    api.merge(SwaggerUi::new("/docs").url("/api-docs/openapi.json", ApiDoc::openapi()))
}

struct BearerAuth;

impl Modify for BearerAuth {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "bearer",
            utoipa::openapi::security::SecurityScheme::Http(
                utoipa::openapi::security::HttpBuilder::new()
                    .scheme(utoipa::openapi::security::HttpAuthScheme::Bearer)
                    .bearer_format("JWT")
                    .description(Some(
                        "Access token from POST /v1/users or POST /v1/auth/refresh.",
                    ))
                    .build(),
            ),
        );
    }
}

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Styrta",
        version = "0.1.0",
        description = "Nearby meetups. Swagger UI is served at /docs. Send `Authorization: Bearer <access_token>` on every route except POST /v1/users and POST /v1/auth/refresh."
    ),
    modifiers(&BearerAuth),
    tags(
        (name = "account", description = "Sign-up and token refresh"),
        (name = "profile", description = "The signed-in person"),
        (name = "events", description = "Public meetups"),
        (name = "chat", description = "Turns, history, and speech"),
        (name = "stream", description = "Server-sent events for one user"),
    ),
    paths(
        create_user,
        refresh_session,
        user_stream,
        get_me,
        patch_me,
        nearby,
        get_event,
        create_event,
        join_event,
        cancel_event,
        complete_event,
        my_events,
        list_messages,
        post_message,
        get_turn,
        get_audio,
    ),
    components(schemas(
        ErrorBody,
        crate::sse::TurnRef,
        crate::sse::TranscriptReady,
        crate::sse::ToolStarted,
        crate::sse::ToolFinished,
        crate::sse::EventDraft,
        crate::sse::ReplyDelta,
        crate::sse::ReplyDone,
        crate::sse::AudioReady,
        crate::sse::TurnFailed,
    ))
)]
struct ApiDoc;

pub fn harness_services(state: &AppState) -> Services<'_> {
    Services {
        store: &state.store,
        model: state.model.as_ref(),
        speech: state.speech.as_ref(),
        embedder: state.embedder.as_ref(),
        geocoder: state.geocoder.as_ref(),
        config: state.config.as_ref(),
    }
}

fn is_public(method: &Method, path: &str) -> bool {
    method == Method::POST && (path == "/v1/users" || path == "/v1/auth/refresh")
}

fn authenticate(secret: &str, authorization: Option<&str>) -> Result<Claims, ApiError> {
    let token = authorization
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .ok_or_else(unauthorized)?;
    auth::verify(secret, token, auth::TokenKind::Access).map_err(|_| unauthorized())
}

async fn require_access(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    if is_public(request.method(), request.uri().path()) {
        return Ok(next.run(request).await);
    }
    let header = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let claims = authenticate(&state.config.jwt_secret, header.as_deref())?;
    request.extensions_mut().insert(claims);
    Ok(next.run(request).await)
}

fn issue_pair(secret: &str, user_id: Uuid) -> Result<(auth::Issued, auth::Issued), ApiError> {
    let access = auth::issue_access(secret, user_id).map_err(|err| {
        tracing::error!(error = %err, "issue access token");
        internal()
    })?;
    let refresh = auth::issue_refresh(secret, user_id).map_err(|err| {
        tracing::error!(error = %err, "issue refresh token");
        internal()
    })?;
    Ok((access, refresh))
}

#[utoipa::path(
    post,
    path = "/v1/users",
    tag = "account",
    request_body = CreateUser,
    responses(
        (status = 200, body = Session),
        (status = 400, description = "Empty display name", body = ErrorBody),
    )
)]
async fn create_user(
    State(state): State<AppState>,
    Json(body): Json<CreateUser>,
) -> Result<Json<Session>, ApiError> {
    let display_name = body.display_name.trim();
    if display_name.is_empty() {
        return Err(bad("display name"));
    }
    let locale = locale_of(body.locale.as_deref());
    let user = state
        .store
        .create_user(display_name, &locale)
        .await
        .map_err(map_store)?;
    let (access, refresh) = issue_pair(&state.config.jwt_secret, user.id)?;
    Ok(Json(Session {
        id: user.id,
        display_name: user.display_name,
        access_token: access.token,
        refresh_token: refresh.token,
        access_expires_at: access.expires_at,
        refresh_expires_at: refresh.expires_at,
    }))
}

#[utoipa::path(
    post,
    path = "/v1/auth/refresh",
    tag = "account",
    request_body = Refresh,
    responses(
        (status = 200, body = Refreshed),
        (status = 401, description = "Refresh token missing or expired", body = ErrorBody),
    )
)]
async fn refresh_session(
    State(state): State<AppState>,
    Json(body): Json<Refresh>,
) -> Result<Json<Refreshed>, ApiError> {
    let claims = auth::verify(
        &state.config.jwt_secret,
        &body.refresh_token,
        auth::TokenKind::Refresh,
    )
    .map_err(|_| unauthorized())?;
    let (access, refresh) = issue_pair(&state.config.jwt_secret, claims.sub)?;
    Ok(Json(Refreshed {
        access_token: access.token,
        refresh_token: refresh.token,
        access_expires_at: access.expires_at,
        refresh_expires_at: refresh.expires_at,
    }))
}

#[utoipa::path(
    get,
    path = "/v1/stream",
    tag = "stream",
    params(
        ("Last-Event-ID" = Option<i64>, Header, description = "Replay events with a greater id, then follow. Omit it to follow only new events.")
    ),
    responses(
        (status = 200, description = "text/event-stream. Each event has an id and a named type: turn.started, transcript.ready, tool.started, tool.finished, event.draft, reply.delta, reply.done, audio.ready, turn.done, turn.failed. Heartbeats are comment lines. event.draft is a public place to pin before the person confirms it. Payload shapes are the components TurnRef, TranscriptReady, ToolStarted, ToolFinished, EventDraft, ReplyDelta, ReplyDone, AudioReady, and TurnFailed.", content_type = "text/event-stream"),
        (status = 401, body = ErrorBody),
    ),
    security(("bearer" = []))
)]
async fn user_stream(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let header = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok());
    let resume = sse::resume_from(header).ok_or_else(|| bad("last-event-id"))?;
    let body = spawn_stream(state, claims.sub, resume);
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header("x-accel-buffering", "no")
        .body(body)
        .map_err(|err| {
            tracing::error!(error = %err, "stream response");
            internal()
        })
}

fn spawn_stream(state: AppState, user_id: Uuid, resume: sse::Resume) -> Body {
    let (tx, rx) = mpsc::channel(32);
    tokio::spawn(async move {
        let mut cursor = match resume {
            sse::Resume::After(id) => id,
            sse::Resume::Live => prime_cursor(&state, user_id).await,
        };
        let mut last_beat = Instant::now();
        let mut warned = false;
        loop {
            if tx.is_closed() {
                break;
            }
            if !poll_once(&state, &tx, user_id, &mut cursor, &mut warned).await {
                break;
            }
            if sse::heartbeat_due(last_beat.elapsed()) {
                if tx
                    .send(Bytes::from_static(sse::HEARTBEAT_COMMENT.as_bytes()))
                    .await
                    .is_err()
                {
                    break;
                }
                last_beat = Instant::now();
            }
            tokio::time::sleep(sse::POLL_INTERVAL).await;
        }
    });
    let stream = ReceiverStream::new(rx).map(Ok::<_, Infallible>);
    Body::from_stream(stream)
}

async fn prime_cursor(state: &AppState, user_id: Uuid) -> i64 {
    match state.store.stream_after(user_id, 0).await {
        Ok(events) => events.into_iter().map(|event| event.id).max().unwrap_or(0),
        Err(err) => {
            tracing::warn!(error = %err, %user_id, "stream prime");
            0
        }
    }
}

async fn poll_once(
    state: &AppState,
    tx: &mpsc::Sender<Bytes>,
    user_id: Uuid,
    cursor: &mut i64,
    warned: &mut bool,
) -> bool {
    let events = match state.store.stream_after(user_id, *cursor).await {
        Ok(events) => events,
        Err(err) => {
            if *warned {
                tracing::debug!(error = %err, %user_id, "stream poll");
            } else {
                tracing::warn!(error = %err, %user_id, "stream poll");
                *warned = true;
            }
            return true;
        }
    };
    let next = events
        .iter()
        .map(|event| event.id)
        .max()
        .map(|id| (*cursor).max(id))
        .unwrap_or(*cursor);
    let tagged = tagged_frames(&events);
    let text = match sse::render(user_id, *cursor, &tagged) {
        Ok(text) => text,
        Err(err) => {
            tracing::warn!(error = %err, %user_id, "encode stream");
            *cursor = next;
            return true;
        }
    };
    if !text.is_empty() && tx.send(Bytes::from(text)).await.is_err() {
        return false;
    }
    *cursor = next;
    true
}

fn tagged_frames(events: &[appdb::StreamEvent]) -> Vec<sse::TaggedFrame> {
    events
        .iter()
        .filter_map(|event| {
            let kind = Kind::parse(&event.kind)?;
            Some(sse::TaggedFrame {
                user_id: event.user_id,
                frame: sse::Frame {
                    id: event.id,
                    kind,
                    data: event.payload.clone(),
                },
            })
        })
        .collect()
}

#[utoipa::path(
    get,
    path = "/v1/me",
    tag = "profile",
    responses((status = 200, body = Me), (status = 401, body = ErrorBody)),
    security(("bearer" = []))
)]
async fn get_me(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<Me>, ApiError> {
    Ok(Json(load_me(&state, claims.sub).await?))
}

#[utoipa::path(
    patch,
    path = "/v1/me",
    tag = "profile",
    request_body = appdb::ProfilePatch,
    responses(
        (status = 200, body = Me),
        (status = 400, body = ErrorBody),
        (status = 401, body = ErrorBody),
    ),
    security(("bearer" = []))
)]
async fn patch_me(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(patch): Json<appdb::ProfilePatch>,
) -> Result<Json<Me>, ApiError> {
    state
        .store
        .apply_profile(claims.sub, &patch)
        .await
        .map_err(map_store)?;
    Ok(Json(load_me(&state, claims.sub).await?))
}

async fn load_me(state: &AppState, user_id: Uuid) -> Result<Me, ApiError> {
    let user = state.store.user(user_id).await.map_err(map_store)?;
    let profile = state.store.profile(user_id).await.map_err(map_store)?;
    Ok(me_from(user, profile))
}

fn me_from(user: appdb::User, profile: appdb::Profile) -> Me {
    Me {
        id: user.id,
        display_name: user.display_name,
        locale: user.locale,
        age_band: profile.age_band,
        gender: profile.gender,
        mobility: profile.mobility,
        sportiness: profile.sportiness,
        women_only: profile.women_only,
        time_window: profile.time_window,
        likes: profile.likes,
        dislikes: profile.dislikes,
    }
}

#[utoipa::path(
    get,
    path = "/v1/events/nearby",
    tag = "events",
    params(NearbyQuery),
    responses(
        (status = 200, description = "Ranked for the caller's stored profile vector. A pan does not embed a new query.", body = Vec<NearbyEvent>),
        (status = 400, body = ErrorBody),
        (status = 401, body = ErrorBody),
    ),
    security(("bearer" = []))
)]
async fn nearby(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Query(query): Query<NearbyQuery>,
) -> Result<Json<Vec<NearbyEvent>>, ApiError> {
    if !query.lat.is_finite() || !query.lng.is_finite() || !query.zoom.is_finite() {
        return Err(bad("query"));
    }
    let radius_m =
        rank::viewport_radius_m(query.lat, query.zoom).map_err(|err| map_rank(err, "zoom"))?;
    let bounds = match query.bbox.as_deref() {
        Some(text) => Some(rank::parse_bbox(text).map_err(|err| map_rank(err, "bbox"))?),
        None => None,
    };
    let now = Utc::now();
    let origin = rank::LatLng {
        lat: query.lat,
        lng: query.lng,
    };
    let candidates = state
        .store
        .nearby_candidates(claims.sub, origin, radius_m, bounds, now)
        .await
        .map_err(map_store)?;
    let profile = state.store.profile(claims.sub).await.map_err(map_store)?;
    let profile = tools::rank_profile(&profile);
    let want = profile.embedding.clone();
    let ranked = rank::rank_events(&rank::RankInput {
        profile: &profile,
        origin,
        radius_m,
        bounds,
        now,
        want_embedding: want.as_deref(),
        events: &candidates,
    })
    .map_err(|err| {
        tracing::error!(error = %err, "rank events");
        internal()
    })?;
    Ok(Json(ranked.into_iter().map(nearby_event).collect()))
}

fn nearby_event(event: rank::ScoredEvent) -> NearbyEvent {
    NearbyEvent {
        id: event.id,
        title: event.title,
        emoji: event.emoji,
        signed_count: event.signed_count,
        capacity: event.capacity,
        place_name: event.place_name,
        latitude: event.latitude,
        longitude: event.longitude,
        starts_at: event.starts_at,
        host_name: event.host_name,
    }
}

#[utoipa::path(
    get,
    path = "/v1/events/{id}",
    tag = "events",
    params(("id" = Uuid, Path, description = "Event id")),
    responses(
        (status = 200, body = EventBody),
        (status = 401, body = ErrorBody),
        (status = 404, body = ErrorBody),
    ),
    security(("bearer" = []))
)]
async fn get_event(
    State(state): State<AppState>,
    Extension(_claims): Extension<Claims>,
    Path(id): Path<Uuid>,
) -> Result<Json<EventBody>, ApiError> {
    let event = state.store.event(id).await.map_err(map_store)?;
    Ok(Json(event_body(event)))
}

#[utoipa::path(
    post,
    path = "/v1/events",
    tag = "events",
    request_body = CreateEvent,
    responses(
        (status = 201, description = "Emoji was supplied, so the event exists now.", body = EventBody),
        (status = 202, description = "Emoji is being chosen. Poll the turn.", body = AcceptedTurn),
        (status = 400, body = ErrorBody),
        (status = 401, body = ErrorBody),
    ),
    security(("bearer" = []))
)]
async fn create_event(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(body): Json<CreateEvent>,
) -> Result<Response, ApiError> {
    validate_event(&body)?;
    let emoji = body.emoji.clone();
    match emoji.as_deref() {
        Some(emoji) if tools::single_grapheme(emoji) => {
            let emoji = emoji.trim().to_string();
            let created = state
                .store
                .create_event(claims.sub, new_event(body, emoji)?)
                .await
                .map_err(map_store)?;
            Ok((StatusCode::CREATED, Json(event_body(created))).into_response())
        }
        Some(_) => Err(bad("emoji must be one grapheme")),
        None => {
            let turn = state
                .store
                .insert_turn(claims.sub)
                .await
                .map_err(map_store)?;
            let turn_id = turn.id;
            let user_id = claims.sub;
            tracing::info!(%turn_id, tool = "create_event", "accepted event");
            let state = state.clone();
            tokio::spawn(async move {
                choose_emoji(state, user_id, turn_id, body).await;
            });
            Ok((StatusCode::ACCEPTED, Json(AcceptedTurn { turn_id })).into_response())
        }
    }
}

async fn choose_emoji(state: AppState, user_id: Uuid, turn_id: Uuid, body: CreateEvent) {
    emit(
        &state,
        user_id,
        turn_id,
        Kind::ToolStarted,
        payload(&sse::ToolStarted {
            turn_id,
            tool: ToolName::CreateEvent.as_str().to_string(),
            label: ToolName::CreateEvent.label().to_string(),
        }),
    )
    .await;
    let emoji = match pick_emoji(&state, &body).await {
        Ok(emoji) => emoji,
        Err(()) => {
            finish_create(&state, user_id, turn_id, false, None).await;
            return;
        }
    };
    let event = new_event(body, emoji);
    let event = match event {
        Ok(event) => event,
        Err(_) => {
            finish_create(&state, user_id, turn_id, false, None).await;
            return;
        }
    };
    match state.store.create_event(user_id, event).await {
        Ok(created) => {
            let event_id = created.id;
            finish_create(&state, user_id, turn_id, true, Some(event_id)).await;
        }
        Err(err) => {
            tracing::warn!(%turn_id, error = %err, "create event");
            finish_create(&state, user_id, turn_id, false, None).await;
        }
    }
}

async fn pick_emoji(state: &AppState, body: &CreateEvent) -> Result<String, ()> {
    let mut request = llm::CompletionRequest::new(vec![llm::Message::User {
        content: format!(
            "Reply with one emoji grapheme and no other text. Title: {}",
            body.title.trim()
        ),
    }]);
    request.temperature = Some(0.0);
    // Reasoning tokens count against this budget. Eight is not enough for the grapheme to land in content.
    request.max_tokens = Some(128);
    let message = match state.model.complete(&request).await {
        Ok(message) => message,
        Err(err) => {
            tracing::warn!(error = %err, "emoji choice");
            return Err(());
        }
    };
    one_emoji(message.content.as_deref().unwrap_or("")).ok_or(())
}

async fn finish_create(
    state: &AppState,
    user_id: Uuid,
    turn_id: Uuid,
    ok: bool,
    event_id: Option<Uuid>,
) {
    tracing::info!(%turn_id, tool = "create_event", ok, "tool finished");
    emit(
        state,
        user_id,
        turn_id,
        Kind::ToolFinished,
        payload(&sse::ToolFinished {
            turn_id,
            tool: ToolName::CreateEvent.as_str().to_string(),
            ok,
            event_id,
            hit_count: None,
        }),
    )
    .await;
    if ok {
        if let Err(err) = state.store.finish_turn(turn_id, "", None).await {
            tracing::warn!(%turn_id, error = %err, "finish create turn");
        }
        emit(
            state,
            user_id,
            turn_id,
            Kind::TurnDone,
            payload(&sse::TurnRef { turn_id }),
        )
        .await;
    } else {
        let message = "could not create the event";
        if let Err(err) = state.store.fail_turn(turn_id, message).await {
            tracing::warn!(%turn_id, error = %err, "fail create turn");
        }
        emit(
            state,
            user_id,
            turn_id,
            Kind::TurnFailed,
            payload(&sse::TurnFailed {
                turn_id,
                error: message.to_string(),
            }),
        )
        .await;
    }
}

#[utoipa::path(
    post,
    path = "/v1/events/{id}/join",
    tag = "events",
    params(("id" = Uuid, Path, description = "Event id")),
    responses(
        (status = 200, body = AttendanceBody),
        (status = 401, body = ErrorBody),
        (status = 404, body = ErrorBody),
        (status = 409, description = "The event is full", body = ErrorBody),
    ),
    security(("bearer" = []))
)]
async fn join_event(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<Uuid>,
) -> Result<Json<AttendanceBody>, ApiError> {
    attendance(
        state
            .store
            .join_event(id, claims.sub)
            .await
            .map_err(map_store)?,
    )
}

#[utoipa::path(
    post,
    path = "/v1/events/{id}/cancel",
    tag = "events",
    params(("id" = Uuid, Path, description = "Event id")),
    responses(
        (status = 200, body = AttendanceBody),
        (status = 401, body = ErrorBody),
        (status = 404, body = ErrorBody),
    ),
    security(("bearer" = []))
)]
async fn cancel_event(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<Uuid>,
) -> Result<Json<AttendanceBody>, ApiError> {
    attendance(
        state
            .store
            .cancel_attendance(id, claims.sub)
            .await
            .map_err(map_store)?,
    )
}

#[utoipa::path(
    post,
    path = "/v1/events/{id}/complete",
    tag = "events",
    params(("id" = Uuid, Path, description = "Event id")),
    responses(
        (status = 200, body = AttendanceBody),
        (status = 401, body = ErrorBody),
        (status = 404, body = ErrorBody),
    ),
    security(("bearer" = []))
)]
async fn complete_event(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<Uuid>,
) -> Result<Json<AttendanceBody>, ApiError> {
    attendance(
        state
            .store
            .complete_attendance(id, claims.sub)
            .await
            .map_err(map_store)?,
    )
}

fn attendance(row: appdb::Attendance) -> Result<Json<AttendanceBody>, ApiError> {
    Ok(Json(AttendanceBody {
        event_id: row.event_id,
        user_id: row.user_id,
        status: row.status,
    }))
}

#[utoipa::path(
    get,
    path = "/v1/me/events",
    tag = "events",
    responses(
        (status = 200, body = Vec<EventBody>),
        (status = 401, body = ErrorBody),
    ),
    security(("bearer" = []))
)]
async fn my_events(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<Vec<EventBody>>, ApiError> {
    let events = state
        .store
        .my_events(claims.sub, Utc::now())
        .await
        .map_err(map_store)?;
    Ok(Json(events.into_iter().map(event_body).collect()))
}

#[utoipa::path(
    get,
    path = "/v1/chat/messages",
    tag = "chat",
    params(MessageQuery),
    responses(
        (status = 200, body = Vec<HistoryMessage>),
        (status = 400, body = ErrorBody),
        (status = 401, body = ErrorBody),
    ),
    security(("bearer" = []))
)]
async fn list_messages(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Query(query): Query<MessageQuery>,
) -> Result<Json<Vec<HistoryMessage>>, ApiError> {
    let limit = history_limit(query.limit)?;
    let messages = state
        .store
        .messages_before(claims.sub, query.before, limit)
        .await
        .map_err(map_store)?;
    Ok(Json(messages.into_iter().map(history_message).collect()))
}

#[utoipa::path(
    post,
    path = "/v1/chat/messages",
    tag = "chat",
    request_body(content = PostMessage, description = "JSON `{ \"text\": \"...\" }`, or multipart/form-data with an `audio` file.", content_type = "application/json"),
    responses(
        (status = 202, body = AcceptedTurn),
        (status = 400, body = ErrorBody),
        (status = 401, body = ErrorBody),
    ),
    security(("bearer" = []))
)]
async fn post_message(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    request: Request,
) -> Result<(StatusCode, Json<AcceptedTurn>), ApiError> {
    let (text, audio) = read_message(request).await?;
    let turn = state
        .store
        .insert_turn(claims.sub)
        .await
        .map_err(map_store)?;
    let turn_id = turn.id;
    let user_id = claims.sub;
    tracing::info!(%turn_id, "accepted turn");
    let state = state.clone();
    tokio::spawn(async move {
        run_chat_turn(state, user_id, turn_id, text, audio).await;
    });
    Ok((StatusCode::ACCEPTED, Json(AcceptedTurn { turn_id })))
}

async fn read_message(
    request: Request,
) -> Result<(Option<String>, Option<(Bytes, String)>), ApiError> {
    let mime = mime_of(request.headers());
    if mime == "application/json" || mime.is_empty() {
        let bytes = axum::body::to_bytes(request.into_body(), JSON_LIMIT)
            .await
            .map_err(|_| bad("body"))?;
        let body: PostMessage = serde_json::from_slice(&bytes).map_err(|_| bad("text"))?;
        let text = body
            .text
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .ok_or_else(|| bad("text"))?
            .to_string();
        Ok((Some(text), None))
    } else if mime.starts_with("multipart/") {
        let audio = read_audio_field(request).await?;
        Ok((None, Some(audio)))
    } else {
        Err(bad("content type"))
    }
}

async fn read_audio_field(request: Request) -> Result<(Bytes, String), ApiError> {
    let mut multipart = axum::extract::Multipart::from_request(request, &())
        .await
        .map_err(|_| bad("audio"))?;
    while let Some(field) = multipart.next_field().await.map_err(|_| bad("audio"))? {
        if field.name() != Some("audio") {
            continue;
        }
        let content_type = field
            .content_type()
            .unwrap_or("application/octet-stream")
            .to_string();
        let bytes = field.bytes().await.map_err(|_| bad("audio"))?;
        if bytes.is_empty() {
            return Err(bad("audio"));
        }
        return Ok((bytes, content_type));
    }
    Err(bad("audio"))
}

async fn run_chat_turn(
    state: AppState,
    user_id: Uuid,
    turn_id: Uuid,
    text: Option<String>,
    audio: Option<(Bytes, String)>,
) {
    let user_text = if let Some((bytes, content_type)) = audio {
        match state.speech.transcribe(bytes, &content_type).await {
            Ok(text) => text,
            Err(err) => {
                tracing::warn!(%turn_id, error = %err, "transcribe");
                fail_chat(&state, user_id, turn_id, "could not transcribe").await;
                return;
            }
        }
    } else {
        text.unwrap_or_default()
    };
    if let Err(err) = state.store.set_turn_user_text(turn_id, &user_text).await {
        tracing::warn!(%turn_id, error = %err, "store user text");
        fail_chat(&state, user_id, turn_id, "could not store user text").await;
        return;
    }
    if let Err(err) = harness::run_turn(harness_services(&state), turn_id).await {
        tracing::warn!(%turn_id, error = %err, "turn");
    }
}

async fn fail_chat(state: &AppState, user_id: Uuid, turn_id: Uuid, message: &str) {
    if let Err(err) = state.store.fail_turn(turn_id, message).await {
        tracing::warn!(%turn_id, error = %err, "fail turn");
    }
    emit(
        state,
        user_id,
        turn_id,
        Kind::TurnFailed,
        payload(&sse::TurnFailed {
            turn_id,
            error: message.to_string(),
        }),
    )
    .await;
}

#[utoipa::path(
    get,
    path = "/v1/chat/turns/{id}",
    tag = "chat",
    params(("id" = Uuid, Path, description = "Turn id")),
    responses(
        (status = 200, body = TurnView),
        (status = 401, body = ErrorBody),
        (status = 404, body = ErrorBody),
    ),
    security(("bearer" = []))
)]
async fn get_turn(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<Uuid>,
) -> Result<Json<TurnView>, ApiError> {
    let turn = load_turn(&state, claims.sub, id).await?;
    let audio_ready = audio_ready(&state.config, &turn);
    Ok(Json(turn_view(turn, audio_ready)))
}

#[utoipa::path(
    get,
    path = "/v1/chat/turns/{id}/audio",
    tag = "chat",
    params(("id" = Uuid, Path, description = "Turn id")),
    responses(
        (status = 200, description = "Spoken reply. Content-Type matches the stored audio.", content_type = "application/octet-stream"),
        (status = 401, body = ErrorBody),
        (status = 404, body = ErrorBody),
    ),
    security(("bearer" = []))
)]
async fn get_audio(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let turn = load_turn(&state, claims.sub, id).await?;
    let stored = turn.audio_path.as_deref().ok_or_else(not_found)?;
    let path = resolve_audio(&state.config, stored);
    let bytes = tokio::fs::read(&path).await.map_err(|_| not_found())?;
    let content_type = audio_type(&path, &bytes);
    Ok(([(header::CONTENT_TYPE, content_type)], bytes).into_response())
}

async fn load_turn(
    state: &AppState,
    user_id: Uuid,
    turn_id: Uuid,
) -> Result<appdb::Turn, ApiError> {
    let turn = state
        .store
        .turn(user_id, turn_id)
        .await
        .map_err(map_store)?;
    if turn.user_id != user_id {
        return Err(not_found());
    }
    Ok(turn)
}

fn turn_view(turn: appdb::Turn, audio_ready: bool) -> TurnView {
    TurnView {
        status: match turn.status {
            appdb::TurnStatus::Running => "running",
            appdb::TurnStatus::Done => "done",
            appdb::TurnStatus::Failed => "failed",
        }
        .to_string(),
        user_text: turn.user_text,
        reply_text: turn.reply_text,
        audio_ready,
    }
}

fn history_message(message: appdb::ChatMessage) -> HistoryMessage {
    HistoryMessage {
        id: message.id,
        role: match message.role {
            appdb::ChatRole::User => "user",
            appdb::ChatRole::Assistant => "assistant",
        }
        .to_string(),
        body: message.body,
        created_at: message.created_at,
    }
}

fn locale_of(locale: Option<&str>) -> String {
    locale
        .map(str::trim)
        .filter(|locale| !locale.is_empty())
        .unwrap_or("pl")
        .to_string()
}

fn history_limit(limit: Option<i64>) -> Result<i64, ApiError> {
    match limit {
        None => Ok(HISTORY_LIMIT),
        Some(limit) if limit > 0 => Ok(limit),
        Some(_) => Err(bad("limit")),
    }
}

fn validate_event(body: &CreateEvent) -> Result<(), ApiError> {
    if !body.lat.is_finite() || !body.lon.is_finite() {
        return Err(bad("coordinates"));
    }
    if matches!(body.capacity, Some(capacity) if capacity < 0) {
        return Err(bad("capacity"));
    }
    parse_kind(&body.kind)?;
    Ok(())
}

fn new_event(body: CreateEvent, emoji: String) -> Result<appdb::NewEvent, ApiError> {
    validate_event(&body)?;
    Ok(appdb::NewEvent {
        title: body.title,
        emoji,
        description: body.description,
        starts_at: body.starts_at,
        capacity: body.capacity,
        activity_tags: body.activity_tags,
        women_only: false,
        place_name: body.place_name,
        place_kind: parse_kind(&body.kind)?,
        latitude: body.lat,
        longitude: body.lon,
    })
}

fn parse_kind(kind: &str) -> Result<rank::PlaceKind, ApiError> {
    let parsed: rank::PlaceKind =
        serde_json::from_value(serde_json::Value::String(kind.to_string()))
            .map_err(|_| bad("kind"))?;
    if parsed.is_public() {
        Ok(parsed)
    } else {
        Err(bad("kind"))
    }
}

fn one_emoji(text: &str) -> Option<String> {
    let text = text
        .trim()
        .trim_matches(|ch| ch == '`' || ch == '"' || ch == '\'')
        .trim();
    if tools::single_grapheme(text) {
        Some(text.to_string())
    } else {
        None
    }
}

fn payload(value: &impl Serialize) -> serde_json::Value {
    serde_json::to_value(value).expect("payload")
}

async fn emit(
    state: &AppState,
    user_id: Uuid,
    turn_id: Uuid,
    kind: Kind,
    payload: serde_json::Value,
) {
    if let Err(err) = state
        .store
        .append_stream(user_id, turn_id, kind.as_str(), payload)
        .await
    {
        tracing::warn!(%turn_id, kind = kind.as_str(), error = %err, "append stream");
    }
}

fn audio_ready(config: &Config, turn: &appdb::Turn) -> bool {
    turn.audio_path
        .as_deref()
        .is_some_and(|stored| resolve_audio(config, stored).is_file())
}

fn resolve_audio(config: &Config, stored: &str) -> std::path::PathBuf {
    let path = std::path::PathBuf::from(stored);
    if path.is_absolute() {
        path
    } else {
        config.audio_dir.join(path)
    }
}

fn audio_type(path: &std::path::Path, bytes: &[u8]) -> &'static str {
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WAVE" {
        return "audio/wav";
    }
    if bytes.len() >= 4 && &bytes[..4] == b"OggS" {
        return "audio/ogg";
    }
    if bytes.len() >= 3 && &bytes[..3] == b"ID3"
        || (bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] & 0xE0 == 0xE0)
    {
        return "audio/mpeg";
    }
    match path.extension().and_then(|ext| ext.to_str()) {
        Some(ext) if ext.eq_ignore_ascii_case("wav") => "audio/wav",
        Some(ext) if ext.eq_ignore_ascii_case("mp3") => "audio/mpeg",
        Some(ext) if ext.eq_ignore_ascii_case("ogg") => "audio/ogg",
        Some(ext) if ext.eq_ignore_ascii_case("flac") => "audio/flac",
        Some(ext) if ext.eq_ignore_ascii_case("m4a") || ext.eq_ignore_ascii_case("mp4") => {
            "audio/mp4"
        }
        _ => "application/octet-stream",
    }
}

fn map_store(err: appdb::Error) -> ApiError {
    let status = store_status(&format!("{err:?} {err}"));
    if status == StatusCode::INTERNAL_SERVER_ERROR {
        tracing::error!(error = %err, "store");
    }
    match status {
        StatusCode::CONFLICT => conflict(),
        StatusCode::NOT_FOUND => not_found(),
        StatusCode::BAD_REQUEST => bad("invalid"),
        _ => internal(),
    }
}

fn store_status(label: &str) -> StatusCode {
    let label: String = label
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(|ch| ch.to_lowercase())
        .collect();
    if label.contains("capacity") || label.contains("full") || label.contains("conflict") {
        StatusCode::CONFLICT
    } else if label.contains("notfound") || label.contains("norows") {
        StatusCode::NOT_FOUND
    } else if label.contains("invalid") {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

fn map_rank(_err: rank::Error, message: &'static str) -> ApiError {
    bad(message)
}

#[derive(Serialize, ToSchema)]
struct EventBody {
    id: Uuid,
    host_id: Uuid,
    host_name: String,
    title: String,
    emoji: String,
    description: Option<String>,
    starts_at: chrono::DateTime<Utc>,
    capacity: Option<i32>,
    activity_tags: Vec<String>,
    promoted: bool,
    status: rank::EventStatus,
    women_only: bool,
    place_name: String,
    place_kind: rank::PlaceKind,
    latitude: f64,
    longitude: f64,
    signed_count: i64,
}

fn event_body(event: appdb::Event) -> EventBody {
    EventBody {
        id: event.id,
        host_id: event.host_id,
        host_name: event.host_name,
        title: event.title,
        emoji: event.emoji,
        description: event.description,
        starts_at: event.starts_at,
        capacity: event.capacity,
        activity_tags: event.activity_tags,
        promoted: event.promoted,
        status: event.status,
        women_only: event.women_only,
        place_name: event.place_name,
        place_kind: event.place_kind,
        latitude: event.latitude,
        longitude: event.longitude,
        signed_count: event.signed_count,
    }
}

#[derive(Serialize, ToSchema)]
struct AttendanceBody {
    event_id: Uuid,
    user_id: Uuid,
    status: appdb::AttendanceStatus,
}

fn mime_of(headers: &HeaderMap) -> String {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::get as route_get;
    use std::time::Duration;

    const SECRET: &str = "test-jwt-secret";

    #[test]
    fn openapi_lists_every_http_route() {
        let spec = ApiDoc::openapi();
        for path in [
            "/v1/users",
            "/v1/auth/refresh",
            "/v1/stream",
            "/v1/me",
            "/v1/events/nearby",
            "/v1/events/{id}",
            "/v1/events",
            "/v1/events/{id}/join",
            "/v1/events/{id}/cancel",
            "/v1/events/{id}/complete",
            "/v1/me/events",
            "/v1/chat/messages",
            "/v1/chat/turns/{id}",
            "/v1/chat/turns/{id}/audio",
        ] {
            assert!(spec.paths.paths.contains_key(path), "{path}");
        }
        let json = spec.to_pretty_json().unwrap();
        assert!(json.contains("bearer"), "{json}");
        assert!(json.contains("turn.started"));
        assert!(json.contains("event.draft"));
    }

    #[test]
    fn refresh_token_on_access_route_is_unauthorized() {
        let refresh = auth::issue_refresh(SECRET, Uuid::from_u128(1)).unwrap();
        let header = format!("Bearer {}", refresh.token);
        let err = authenticate(SECRET, Some(&header)).unwrap_err();
        assert_eq!(err.status, StatusCode::UNAUTHORIZED);
        assert_eq!(err.into_response().status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn access_token_yields_sub() {
        let user = Uuid::from_u128(9);
        let access = auth::issue_access(SECRET, user).unwrap();
        let header = format!("Bearer {}", access.token);
        let claims = authenticate(SECRET, Some(&header)).unwrap();
        assert_eq!(claims.sub, user);
        assert_eq!(claims.typ, auth::TokenKind::Access);
    }

    #[test]
    fn missing_or_bad_bearer_is_unauthorized() {
        assert_eq!(
            authenticate(SECRET, None).unwrap_err().status,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            authenticate(SECRET, Some("Bearer")).unwrap_err().status,
            StatusCode::UNAUTHORIZED
        );
        let access = auth::issue_access("other-secret", Uuid::from_u128(1)).unwrap();
        let header = format!("Bearer {}", access.token);
        assert_eq!(
            authenticate(SECRET, Some(&header)).unwrap_err().status,
            StatusCode::UNAUTHORIZED
        );
    }

    #[test]
    fn only_create_user_and_refresh_are_public() {
        assert!(is_public(&Method::POST, "/v1/users"));
        assert!(is_public(&Method::POST, "/v1/auth/refresh"));
        assert!(!is_public(&Method::GET, "/v1/users"));
        assert!(!is_public(&Method::GET, "/v1/auth/refresh"));
        for path in [
            "/v1/stream",
            "/v1/me",
            "/v1/events/nearby",
            "/v1/events/00000000-0000-0000-0000-000000000000",
            "/v1/events",
            "/v1/events/00000000-0000-0000-0000-000000000000/join",
            "/v1/me/events",
            "/v1/chat/messages",
            "/v1/chat/turns/00000000-0000-0000-0000-000000000000",
            "/v1/chat/turns/00000000-0000-0000-0000-000000000000/audio",
        ] {
            assert!(!is_public(&Method::GET, path), "{path}");
            assert!(!is_public(&Method::POST, path), "{path}");
            assert!(!is_public(&Method::PATCH, path), "{path}");
        }
    }

    #[test]
    fn store_errors_map_to_http_status() {
        assert_eq!(store_status("AtCapacity"), StatusCode::CONFLICT);
        assert_eq!(store_status("event is at capacity"), StatusCode::CONFLICT);
        assert_eq!(store_status("Full"), StatusCode::CONFLICT);
        assert_eq!(store_status("NotFound"), StatusCode::NOT_FOUND);
        assert_eq!(store_status("no rows"), StatusCode::NOT_FOUND);
        assert_eq!(store_status("Database"), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            map_store(appdb::Error::Capacity).status,
            StatusCode::CONFLICT
        );
        assert_eq!(
            map_store(appdb::Error::NotFound).status,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            map_store(appdb::Error::Invalid).status,
            StatusCode::BAD_REQUEST
        );
    }

    #[test]
    fn locale_defaults_to_polish() {
        assert_eq!(locale_of(None), "pl");
        assert_eq!(locale_of(Some("  ")), "pl");
        assert_eq!(locale_of(Some("en")), "en");
    }

    #[test]
    fn history_limit_defaults_to_three() {
        assert_eq!(history_limit(None).unwrap(), 3);
        assert_eq!(history_limit(Some(1)).unwrap(), 1);
        assert!(history_limit(Some(0)).is_err());
        assert!(history_limit(Some(-3)).is_err());
    }

    #[test]
    fn emoji_must_be_one_grapheme() {
        assert_eq!(one_emoji("🎉"), Some("🎉".to_string()));
        assert_eq!(one_emoji("  \"🎉\"  "), Some("🎉".to_string()));
        assert_eq!(one_emoji("🎉 party"), None);
        assert_eq!(one_emoji(""), None);
    }

    #[test]
    fn wav_bytes_set_audio_content_type() {
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&[0, 0, 0, 0]);
        bytes.extend_from_slice(b"WAVE");
        let path = std::path::Path::new("reply.bin");
        assert_eq!(audio_type(path, &bytes), "audio/wav");
        assert_eq!(
            audio_type(std::path::Path::new("reply.mp3"), b"not-audio"),
            "audio/mpeg"
        );
    }

    #[tokio::test]
    async fn swagger_ui_and_openapi_json_are_public() {
        let server = running().await;
        let client = http();
        let docs = client
            .get(format!("{}/docs", server.base))
            .send()
            .await
            .unwrap();
        assert!(docs.status().is_success(), "{}", docs.status());
        let docs_type = docs
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_string();
        let docs_body = docs.text().await.unwrap();
        assert!(
            docs_type.contains("text/html") || docs_body.contains("swagger"),
            "{docs_type} {docs_body}"
        );
        let spec = client
            .get(format!("{}/api-docs/openapi.json", server.base))
            .send()
            .await
            .unwrap();
        assert_eq!(spec.status(), StatusCode::OK);
        let body = spec.text().await.unwrap();
        assert!(body.contains("/v1/events/nearby"), "{body}");
        assert!(body.contains("/v1/chat/messages"), "{body}");
        assert!(body.contains("bearer"), "{body}");
    }

    #[tokio::test]
    async fn access_route_rejects_refresh_token_over_http() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new().route(
            "/v1/me",
            route_get(|headers: HeaderMap| async move {
                let header = headers
                    .get(header::AUTHORIZATION)
                    .and_then(|value| value.to_str().ok());
                match authenticate(SECRET, header) {
                    Ok(_) => StatusCode::OK,
                    Err(err) => err.into_response().status(),
                }
            }),
        );
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let refresh = auth::issue_refresh(SECRET, Uuid::from_u128(3)).unwrap();
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        let response = client
            .get(format!("http://{addr}/v1/me"))
            .header(
                header::AUTHORIZATION.as_str(),
                format!("Bearer {}", refresh.token),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn rewrites_only_the_database_name() {
        let url = "postgres://user:secret@127.0.0.1:5432/styrta?sslmode=disable";
        assert_eq!(
            with_database(url, "styrta_http_test"),
            "postgres://user:secret@127.0.0.1:5432/styrta_http_test?sslmode=disable"
        );
    }

    struct Running {
        base: String,
        state: AppState,
    }

    fn db_lock() -> &'static tokio::sync::Mutex<()> {
        static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
        &LOCK
    }

    async fn running() -> &'static Running {
        static SERVER: tokio::sync::OnceCell<Running> = tokio::sync::OnceCell::const_new();
        SERVER.get_or_init(boot).await
    }

    async fn boot() -> Running {
        let env_file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".env");
        let _ = dotenvy::from_filename(env_file);
        let base_url =
            std::env::var("DATABASE_URL").unwrap_or_else(|_| panic!("DATABASE_URL is not set"));
        let test_url = prepare_database(&base_url).await;
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let thread_url = test_url.clone();
        let thread_base = base_url.clone();
        // tokio::test drops its runtime when the first test returns. The listener
        // and pool have to live on a runtime that outlasts that test.
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let store = Store::connect(&thread_url).await.unwrap_or_else(|err| {
                    panic!("{}", scrub(&err.to_string(), &[&thread_base, &thread_url]))
                });
                let config = test_config(&thread_url);
                let state = AppState {
                    config: Arc::new(config),
                    store,
                    model: Arc::new(ScriptedModel),
                    speech: Arc::new(ScriptedSpeech),
                    embedder: Arc::new(ScriptedEmbedder),
                    geocoder: Arc::new(ScriptedGeocoder),
                };
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let addr = listener.local_addr().unwrap();
                let app = router(state.clone());
                tx.send((format!("http://{addr}"), state)).unwrap();
                axum::serve(listener, app).await.unwrap();
            });
        });
        let (base, state) = tokio::task::spawn_blocking(move || rx.recv().unwrap())
            .await
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            if http().get(format!("{base}/v1/me")).send().await.is_ok() {
                break;
            }
            if std::time::Instant::now() > deadline {
                panic!("server did not accept connections");
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Running { base, state }
    }

    fn test_config(database_url: &str) -> Config {
        let audio = "/tmp/styrta-http-audio";
        std::fs::create_dir_all(audio).unwrap();
        let values = [
            ("LLM_BASE_URL", "http://127.0.0.1:9"),
            ("LLM_API_KEY", "test-llm-key"),
            ("LLM_MODEL", "test-model"),
            ("SPEECH_BASE_URL", "http://127.0.0.1:9"),
            ("STT_MODEL", "stt"),
            ("TTS_MODEL", "tts"),
            ("TTS_VOICE", "voice"),
            ("EMBED_BASE_URL", "http://127.0.0.1:9"),
            ("EMBED_MODEL", "embed"),
            ("EMBED_QUERY_PREFIX", "query: "),
            ("EMBED_PASSAGE_PREFIX", "passage: "),
            ("STYRTA_JWT_SECRET", SECRET),
            ("STYRTA_AUDIO_DIR", audio),
            ("DATABASE_URL", database_url),
        ];
        Config::from_lookup(|key| {
            values
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| (*value).to_string())
        })
        .unwrap_or_else(|err| panic!("{}", scrub(&format!("{err:#}"), &[database_url])))
    }

    async fn prepare_database(base_url: &str) -> String {
        let test_url = with_database(base_url, "styrta_http_test");
        let admin = admin_pool(base_url).await;
        let urls = [base_url, test_url.as_str()];
        exec_raw(
            &admin,
            "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = 'styrta_http_test' AND pid <> pg_backend_pid()",
            &urls,
        )
        .await;
        exec_raw(&admin, "DROP DATABASE IF EXISTS styrta_http_test", &urls).await;
        exec_raw(&admin, "CREATE DATABASE styrta_http_test", &urls).await;
        test_url
    }

    async fn admin_pool(base_url: &str) -> sqlx::PgPool {
        let candidates = [with_database(base_url, "postgres"), base_url.to_string()];
        let mut last = String::from("connect admin");
        for url in &candidates {
            match sqlx::PgPool::connect(url).await {
                Ok(pool) => return pool,
                Err(err) => last = scrub(&err.to_string(), &[base_url, url]),
            }
        }
        panic!("{last}");
    }

    async fn exec_raw(pool: &sqlx::PgPool, sql: &str, urls: &[&str]) {
        if let Err(err) = sqlx::raw_sql(sql).execute(pool).await {
            panic!("{}", scrub(&err.to_string(), urls));
        }
    }

    fn with_database(url: &str, name: &str) -> String {
        let (without_query, query) = match url.split_once('?') {
            Some((base, query)) => (base, Some(query)),
            None => (url, None),
        };
        let slash = without_query.rfind('/').expect("database url has a name");
        let mut out = format!("{}{name}", &without_query[..=slash]);
        if let Some(query) = query {
            out.push('?');
            out.push_str(query);
        }
        out
    }

    fn scrub(text: &str, urls: &[&str]) -> String {
        let mut out = text.to_string();
        for url in urls {
            if !url.is_empty() {
                out = out.replace(url, "DATABASE_URL");
                if let Some(password) = password_of(url) {
                    out = out.replace(password, "***");
                }
            }
        }
        out
    }

    fn password_of(url: &str) -> Option<&str> {
        let rest = url.split_once("://")?.1;
        let userinfo = rest.split_once('@')?.0;
        let password = userinfo.split_once(':')?.1;
        if password.is_empty() {
            None
        } else {
            Some(password)
        }
    }

    fn http() -> reqwest::Client {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap()
    }

    async fn create_session(base: &str, name: &str) -> serde_json::Value {
        let response = http()
            .post(format!("{base}/v1/users"))
            .json(&serde_json::json!({ "display_name": name }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: serde_json::Value = response.json().await.unwrap();
        assert!(body["access_token"].as_str().unwrap().starts_with("ey"));
        assert_eq!(body["display_name"], name);
        body
    }

    async fn get_json(base: &str, path: &str, token: &str) -> (StatusCode, serde_json::Value) {
        let response = http()
            .get(format!("{base}{path}"))
            .bearer_auth(token)
            .send()
            .await
            .unwrap();
        let status = response.status();
        let body = response.json().await.unwrap_or(serde_json::json!({}));
        (status, body)
    }

    #[tokio::test]
    async fn router_session_profile_and_refresh_are_gated() {
        let _guard = db_lock().lock().await;
        let app = running().await;
        let missing = http()
            .get(format!("{}/v1/me", app.base))
            .send()
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);

        let session = create_session(&app.base, "Ada").await;
        let access = session["access_token"].as_str().unwrap();
        let refresh = session["refresh_token"].as_str().unwrap();
        let denied = http()
            .get(format!("{}/v1/me", app.base))
            .bearer_auth(refresh)
            .send()
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);

        let (status, me) = get_json(&app.base, "/v1/me", access).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(me["locale"], "pl");
        assert_eq!(me["display_name"], "Ada");
        assert_eq!(me["likes"], serde_json::json!([]));

        let patched = http()
            .patch(format!("{}/v1/me", app.base))
            .bearer_auth(access)
            .json(&serde_json::json!({
                "display_name": "Ada Lovelace",
                "locale": "en",
                "sportiness": 2
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(patched.status(), StatusCode::OK);
        let patched: serde_json::Value = patched.json().await.unwrap();
        assert_eq!(patched["display_name"], "Ada Lovelace");
        assert_eq!(patched["locale"], "en");
        assert_eq!(patched["sportiness"], 2);

        let refreshed = http()
            .post(format!("{}/v1/auth/refresh", app.base))
            .json(&serde_json::json!({ "refresh_token": refresh }))
            .send()
            .await
            .unwrap();
        assert_eq!(refreshed.status(), StatusCode::OK);
        let refreshed: serde_json::Value = refreshed.json().await.unwrap();
        let (status, me) = get_json(
            &app.base,
            "/v1/me",
            refreshed["access_token"].as_str().unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(me["display_name"], "Ada Lovelace");
    }

    #[tokio::test]
    async fn router_events_join_capacity_and_nearby() {
        let _guard = db_lock().lock().await;
        let app = running().await;
        let host = create_session(&app.base, "Host").await;
        let guest = create_session(&app.base, "Guest").await;
        let extra = create_session(&app.base, "Extra").await;
        let host_token = host["access_token"].as_str().unwrap();
        let guest_token = guest["access_token"].as_str().unwrap();
        let extra_token = extra["access_token"].as_str().unwrap();
        let starts = (chrono::Utc::now() + chrono::Duration::hours(2)).to_rfc3339();
        let created = http()
            .post(format!("{}/v1/events", app.base))
            .bearer_auth(host_token)
            .json(&serde_json::json!({
                "title": "Park walk",
                "emoji": "🎉",
                "starts_at": starts,
                "place_name": "Lazienki",
                "kind": "park",
                "lat": 52.215,
                "lon": 21.035,
                "capacity": 1
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::CREATED);
        let created: serde_json::Value = created.json().await.unwrap();
        let event_id = created["id"].as_str().unwrap();
        assert_eq!(created["emoji"], "🎉");

        let bad_emoji = http()
            .post(format!("{}/v1/events", app.base))
            .bearer_auth(host_token)
            .json(&serde_json::json!({
                "title": "Nope",
                "emoji": "no",
                "starts_at": starts,
                "place_name": "Lazienki",
                "kind": "park",
                "lat": 52.215,
                "lon": 21.035
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(bad_emoji.status(), StatusCode::BAD_REQUEST);

        let (status, fetched) =
            get_json(&app.base, &format!("/v1/events/{event_id}"), host_token).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(fetched["title"], "Park walk");

        let joined = http()
            .post(format!("{}/v1/events/{event_id}/join", app.base))
            .bearer_auth(guest_token)
            .send()
            .await
            .unwrap();
        assert_eq!(joined.status(), StatusCode::OK);
        let joined: serde_json::Value = joined.json().await.unwrap();
        assert_eq!(joined["status"], "going");

        let (status, mine) = get_json(&app.base, "/v1/me/events", guest_token).await;
        assert_eq!(status, StatusCode::OK);
        assert!(mine
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["id"] == event_id));

        let full = http()
            .post(format!("{}/v1/events/{event_id}/join", app.base))
            .bearer_auth(extra_token)
            .send()
            .await
            .unwrap();
        assert_eq!(full.status(), StatusCode::CONFLICT);

        let cancelled = http()
            .post(format!("{}/v1/events/{event_id}/cancel", app.base))
            .bearer_auth(guest_token)
            .send()
            .await
            .unwrap();
        assert_eq!(cancelled.status(), StatusCode::OK);
        http()
            .post(format!("{}/v1/events/{event_id}/join", app.base))
            .bearer_auth(guest_token)
            .send()
            .await
            .unwrap();
        let completed = http()
            .post(format!("{}/v1/events/{event_id}/complete", app.base))
            .bearer_auth(guest_token)
            .send()
            .await
            .unwrap();
        assert_eq!(completed.status(), StatusCode::OK);
        let completed: serde_json::Value = completed.json().await.unwrap();
        assert_eq!(completed["status"], "completed");
        let (_status, mine) = get_json(&app.base, "/v1/me/events", guest_token).await;
        assert!(!mine
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["id"] == event_id));

        let (status, nearby) = get_json(
            &app.base,
            "/v1/events/nearby?lat=52.215&lng=21.035&zoom=14",
            host_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{nearby}");
        assert!(nearby
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["id"] == event_id));

        let (status, outside) = get_json(
            &app.base,
            "/v1/events/nearby?lat=52.215&lng=21.035&zoom=14&bbox=0,0,1,1",
            host_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(!outside
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["id"] == event_id));

        let bad_bbox = http()
            .get(format!(
                "{}/v1/events/nearby?lat=52.2&lng=21.0&zoom=14&bbox=nope",
                app.base
            ))
            .bearer_auth(host_token)
            .send()
            .await
            .unwrap();
        assert_eq!(bad_bbox.status(), StatusCode::BAD_REQUEST);

        let accepted = http()
            .post(format!("{}/v1/events", app.base))
            .bearer_auth(host_token)
            .json(&serde_json::json!({
                "title": "Pick one",
                "starts_at": starts,
                "place_name": "Lazienki",
                "kind": "park",
                "lat": 52.215,
                "lon": 21.035
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(accepted.status(), StatusCode::ACCEPTED);
        let accepted: serde_json::Value = accepted.json().await.unwrap();
        let turn_id = accepted["turn_id"].as_str().unwrap().to_string();
        let body = read_sse(&app.base, host_token, Some(0), |text| {
            text.contains("tool.finished") && text.contains(&turn_id)
        })
        .await;
        assert!(body.contains("event: tool.started"), "{body}");
        assert!(body.contains("event: tool.finished"), "{body}");
        assert!(!body.contains("bravo-secret"));
        let event_id = data_values(&body)
            .into_iter()
            .find(|value| value["ok"] == true && value["turn_id"] == turn_id)
            .and_then(|value| value["event_id"].as_str().map(str::to_string))
            .unwrap_or_else(|| panic!("missing event id in {body}"));
        let (status, picked) =
            get_json(&app.base, &format!("/v1/events/{event_id}"), host_token).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(picked["emoji"], "🎉");
    }

    #[tokio::test]
    async fn router_chat_audio_and_stream_stay_on_one_user() {
        let _guard = db_lock().lock().await;
        let app = running().await;
        let ada = create_session(&app.base, "StreamAda").await;
        let bob = create_session(&app.base, "StreamBob").await;
        let ada_token = ada["access_token"].as_str().unwrap();
        let bob_token = bob["access_token"].as_str().unwrap();
        let ada_id = Uuid::parse_str(ada["id"].as_str().unwrap()).unwrap();
        let ada_turn = post_text(&app.base, ada_token, "first note").await;
        let ada_turn_2 = post_text(&app.base, ada_token, "second note").await;
        let bob_turn = post_text(&app.base, bob_token, "bob secret note").await;
        let audio = http()
            .post(format!("{}/v1/chat/messages", app.base))
            .bearer_auth(ada_token)
            .multipart(
                reqwest::multipart::Form::new().part(
                    "audio",
                    reqwest::multipart::Part::bytes(vec![1, 2, 3, 4])
                        .file_name("note.wav")
                        .mime_str("audio/wav")
                        .unwrap(),
                ),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(audio.status(), StatusCode::ACCEPTED);
        let audio: serde_json::Value = audio.json().await.unwrap();
        let audio_turn = audio["turn_id"].as_str().unwrap().to_string();

        let ada_view = wait_turn(&app.base, ada_token, &ada_turn, "failed").await;
        assert_eq!(ada_view["user_text"], "first note");
        let audio_view = wait_turn(&app.base, ada_token, &audio_turn, "failed").await;
        assert_eq!(audio_view["user_text"], "from-audio");
        let (status, _) =
            get_json(&app.base, &format!("/v1/chat/turns/{bob_turn}"), ada_token).await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        app.state
            .store
            .insert_message(ada_id, appdb::ChatRole::User, "stored hello")
            .await
            .unwrap();
        let (status, history) = get_json(&app.base, "/v1/chat/messages", ada_token).await;
        assert_eq!(status, StatusCode::OK);
        assert!(history
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["body"] == "stored hello" && message["role"] == "user"));

        let replay = read_sse(&app.base, ada_token, Some(0), |text| {
            text.contains(&ada_turn) && text.contains(&ada_turn_2)
        })
        .await;
        assert!(replay.contains(&ada_turn), "{replay}");
        assert!(replay.contains(&ada_turn_2), "{replay}");
        assert!(
            !replay.contains(&bob_turn),
            "user B leaked into the stream: {replay}"
        );
        let ids = sse_ids(&replay);
        assert!(ids.len() >= 2, "{replay}");
        let cursor = ids[0];
        let tail = read_sse(&app.base, ada_token, Some(cursor), |text| {
            sse_ids(text).iter().any(|id| *id > cursor)
        })
        .await;
        assert!(
            sse_ids(&tail).iter().all(|id| *id > cursor),
            "replayed {cursor}: {tail}"
        );
        assert!(!tail.contains(&bob_turn), "{tail}");

        let stored = app.state.store.insert_turn(ada_id).await.unwrap();
        let path = std::path::PathBuf::from(format!("/tmp/styrta-http-audio/{}.wav", stored.id));
        let mut wav = b"RIFF".to_vec();
        wav.extend_from_slice(&0u32.to_le_bytes());
        wav.extend_from_slice(b"WAVE");
        std::fs::write(&path, &wav).unwrap();
        app.state
            .store
            .finish_turn(stored.id, "played", Some(path.to_str().unwrap()))
            .await
            .unwrap();
        let (status, view) = get_json(
            &app.base,
            &format!("/v1/chat/turns/{}", stored.id),
            ada_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(view["audio_ready"], true);
        let bytes = http()
            .get(format!("{}/v1/chat/turns/{}/audio", app.base, stored.id))
            .bearer_auth(ada_token)
            .send()
            .await
            .unwrap();
        assert_eq!(bytes.status(), StatusCode::OK);
        assert_eq!(
            bytes
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("audio/wav")
        );
        assert_eq!(bytes.bytes().await.unwrap().as_ref(), wav.as_slice());
        let foreign_audio = http()
            .get(format!("{}/v1/chat/turns/{}/audio", app.base, stored.id))
            .bearer_auth(bob_token)
            .send()
            .await
            .unwrap();
        assert_eq!(foreign_audio.status(), StatusCode::NOT_FOUND);
    }

    async fn post_text(base: &str, token: &str, text: &str) -> String {
        let response = http()
            .post(format!("{base}/v1/chat/messages"))
            .bearer_auth(token)
            .json(&serde_json::json!({ "text": text }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body: serde_json::Value = response.json().await.unwrap();
        body["turn_id"].as_str().unwrap().to_string()
    }

    async fn wait_turn(base: &str, token: &str, turn_id: &str, status: &str) -> serde_json::Value {
        let deadline = std::time::Instant::now() + Duration::from_secs(4);
        loop {
            let (code, body) = get_json(base, &format!("/v1/chat/turns/{turn_id}"), token).await;
            if code == StatusCode::OK && body["status"] == status {
                return body;
            }
            if std::time::Instant::now() > deadline {
                panic!("turn {turn_id} stayed {body}");
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    }

    async fn read_sse(
        base: &str,
        token: &str,
        last_id: Option<i64>,
        stop: impl Fn(&str) -> bool,
    ) -> String {
        let mut request = http().get(format!("{base}/v1/stream")).bearer_auth(token);
        if let Some(id) = last_id {
            request = request.header("last-event-id", id.to_string());
        }
        let mut response = request.send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        assert!(
            content_type.starts_with("text/event-stream"),
            "{content_type}"
        );
        let mut buf = String::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(4);
        while std::time::Instant::now() < deadline && !stop(&buf) {
            match tokio::time::timeout(Duration::from_millis(500), response.chunk()).await {
                Ok(Ok(Some(chunk))) => buf.push_str(&String::from_utf8_lossy(&chunk)),
                Ok(Ok(None)) => break,
                _ => {}
            }
        }
        buf
    }

    fn sse_ids(body: &str) -> Vec<i64> {
        body.lines()
            .filter_map(|line| line.strip_prefix("id: ")?.trim().parse().ok())
            .collect()
    }

    fn data_values(body: &str) -> Vec<serde_json::Value> {
        body.lines()
            .filter_map(|line| serde_json::from_str(line.strip_prefix("data: ")?).ok())
            .collect()
    }

    struct ScriptedModel;

    #[async_trait::async_trait]
    impl Model for ScriptedModel {
        async fn complete(
            &self,
            _request: &llm::CompletionRequest,
        ) -> anyhow::Result<llm::AssistantMessage> {
            Ok(llm::AssistantMessage {
                content: Some("🎉".to_string()),
                tool_calls: Vec::new(),
            })
        }

        async fn stream(
            &self,
            _request: &llm::CompletionRequest,
        ) -> anyhow::Result<llm::ModelStream> {
            Err(anyhow::anyhow!("stream unused"))
        }
    }

    struct ScriptedSpeech;

    #[async_trait::async_trait]
    impl Speech for ScriptedSpeech {
        async fn transcribe(
            &self,
            _audio: Bytes,
            _content_type: &str,
        ) -> Result<String, crate::speech::Error> {
            Ok("from-audio".to_string())
        }

        async fn speak(&self, _text: &str) -> Result<Bytes, crate::speech::Error> {
            Err(crate::speech::Error::EmptyBody)
        }
    }

    struct ScriptedEmbedder;

    #[async_trait::async_trait]
    impl Embedder for ScriptedEmbedder {
        async fn embed(
            &self,
            _input: crate::embed::Input,
            texts: &[String],
        ) -> Result<Vec<Vec<f32>>, crate::embed::Error> {
            Ok(texts.iter().map(|_| vec![0.0; 1024]).collect())
        }
    }

    struct ScriptedGeocoder;

    #[async_trait::async_trait]
    impl crate::places::Geocoder for ScriptedGeocoder {
        async fn search(
            &self,
            _query: &str,
        ) -> Result<crate::places::PlaceSearch, crate::places::Error> {
            Ok(crate::places::PlaceSearch {
                places: Vec::new(),
                rejected_private: false,
            })
        }
    }
}
