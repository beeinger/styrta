use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use unicode_segmentation::UnicodeSegmentation;
use uuid::Uuid;

use crate::appdb::{
    Attendance, Event, ForgetResult, Memory, NewEvent, NewMemory, Profile, ProfilePatch,
};
use crate::embed::{Embedder, Input};
use crate::harness::{HarnessStore, StoreError};
use crate::knowledge::{Hit, Query};
use crate::llm::{ToolCall, ToolDefinition};
use crate::rank::{self, PersonHit, PlaceKind, ScoredEvent};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    BadArguments(String),
    Failed(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadArguments(message) | Self::Failed(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for Error {}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolName {
    Remember,
    Forget,
    SetProfile,
    SearchKnowledge,
    SearchEvents,
    SearchPeople,
    ListMyEvents,
    CreateEvent,
    JoinEvent,
    CancelAttendance,
    CompleteAttendance,
    SearchChatHistory,
}

impl ToolName {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Remember => "remember",
            Self::Forget => "forget",
            Self::SetProfile => "set_profile",
            Self::SearchKnowledge => "search_knowledge",
            Self::SearchEvents => "search_events",
            Self::SearchPeople => "search_people",
            Self::ListMyEvents => "list_my_events",
            Self::CreateEvent => "create_event",
            Self::JoinEvent => "join_event",
            Self::CancelAttendance => "cancel_attendance",
            Self::CompleteAttendance => "complete_attendance",
            Self::SearchChatHistory => "search_chat_history",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "remember" => Self::Remember,
            "forget" => Self::Forget,
            "set_profile" => Self::SetProfile,
            "search_knowledge" => Self::SearchKnowledge,
            "search_events" => Self::SearchEvents,
            "search_people" => Self::SearchPeople,
            "list_my_events" => Self::ListMyEvents,
            "create_event" => Self::CreateEvent,
            "join_event" => Self::JoinEvent,
            "cancel_attendance" => Self::CancelAttendance,
            "complete_attendance" => Self::CompleteAttendance,
            "search_chat_history" => Self::SearchChatHistory,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Remember => "Saving a memory",
            Self::Forget => "Forgetting",
            Self::SetProfile => "Updating your profile",
            Self::SearchKnowledge => "Searching the library",
            Self::SearchEvents => "Searching events",
            Self::SearchPeople => "Searching people",
            Self::ListMyEvents => "Listing your events",
            Self::CreateEvent => "Creating the event",
            Self::JoinEvent => "Joining the event",
            Self::CancelAttendance => "Cancelling",
            Self::CompleteAttendance => "Completing the event",
            Self::SearchChatHistory => "Searching the chat",
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct ForgetArgs {
    pub topic: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SearchKnowledgeArgs {
    pub query: String,
    pub category: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SearchEventsArgs {
    pub query: Option<String>,
    pub lat: Option<f64>,
    pub lng: Option<f64>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SearchPeopleArgs {
    pub query: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct PeopleLocation {
    lat: Option<f64>,
    lng: Option<f64>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct EventIdArgs {
    pub event_id: Uuid,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SearchChatHistoryArgs {
    pub query: String,
}

#[derive(Deserialize)]
struct RememberArgs {
    durability: crate::appdb::Durability,
    key: String,
    value: String,
    #[serde(default)]
    quote: Option<String>,
    #[serde(default)]
    confidence: Option<f32>,
}

#[derive(Deserialize)]
struct CreateEventArgs {
    title: String,
    emoji: String,
    starts_at: DateTime<Utc>,
    place_name: String,
    kind: PlaceKind,
    lat: f64,
    lon: f64,
    #[serde(default)]
    capacity: Option<i32>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    activity_tags: Vec<String>,
    #[serde(default)]
    women_only: bool,
}

pub struct ToolContext<'a, S = crate::appdb::Store> {
    pub store: &'a S,
    pub embedder: &'a dyn Embedder,
    pub user_id: Uuid,
    pub now: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolResult {
    pub body: Value,
    pub event_id: Option<Uuid>,
    pub hit_count: Option<u32>,
}

impl ToolResult {
    fn plain(body: Value) -> Self {
        Self {
            body,
            event_id: None,
            hit_count: None,
        }
    }

    fn hits(body: Value, count: usize) -> Self {
        Self {
            body,
            event_id: None,
            hit_count: Some(u32::try_from(count).unwrap_or(u32::MAX)),
        }
    }

    fn event(body: Value, event_id: Uuid) -> Self {
        Self {
            body,
            event_id: Some(event_id),
            hit_count: None,
        }
    }
}

pub fn single_grapheme(text: &str) -> bool {
    let mut graphemes = text.trim().graphemes(true);
    graphemes.next().is_some() && graphemes.next().is_none()
}

pub fn specs() -> Result<Vec<ToolDefinition>, Error> {
    Ok(vec![
        spec(
            ToolName::Remember,
            "Store a fact the user just told you.",
            schema(
                json!({
                    "durability": {"type": "string", "enum": ["long_term", "short_term"]},
                    "key": {"type": "string"},
                    "value": {"type": "string"},
                    "quote": {"type": "string"},
                    "confidence": {"type": "number"},
                }),
                &["durability", "key", "value"],
            ),
        ),
        spec(
            ToolName::Forget,
            "Forget a topic the user named. Pass their words, not a row id.",
            schema(json!({"topic": {"type": "string"}}), &["topic"]),
        ),
        spec(
            ToolName::SetProfile,
            "Update any subset of the user's profile.",
            schema(
                json!({
                    "display_name": {"type": "string"},
                    "locale": {"type": "string"},
                    "age_band": {"type": "string"},
                    "gender": {"type": "string"},
                    "mobility": {"type": "string"},
                    "sportiness": {"type": "integer"},
                    "women_only": {"type": "boolean"},
                    "time_window": {
                        "type": "object",
                        "properties": {
                            "start_minute": {"type": "integer"},
                            "end_minute": {"type": "integer"},
                        },
                        "required": ["start_minute", "end_minute"],
                    },
                    "like_tag": {"type": "string"},
                    "dislike_tag": {"type": "string"},
                    "bio": {"type": "string"},
                }),
                &[],
            ),
        ),
        spec(
            ToolName::SearchKnowledge,
            "Search the library. An empty result means the library has nothing.",
            schema(
                json!({
                    "query": {"type": "string"},
                    "category": {"type": "string"},
                }),
                &["query"],
            ),
        ),
        spec(
            ToolName::SearchEvents,
            "Search nearby events with the same ranking as the map.",
            schema(
                json!({
                    "query": {"type": "string"},
                    "lat": {"type": "number"},
                    "lng": {"type": "number"},
                }),
                &["lat", "lng"],
            ),
        ),
        spec(
            ToolName::SearchPeople,
            "Search people near a location. Results omit other people's private notes.",
            schema(
                json!({
                    "query": {"type": "string"},
                    "lat": {"type": "number"},
                    "lng": {"type": "number"},
                }),
                &["lat", "lng"],
            ),
        ),
        spec(
            ToolName::ListMyEvents,
            "List events the user is going to.",
            schema(json!({}), &[]),
        ),
        spec(
            ToolName::CreateEvent,
            "Create an event after the user asked. Emoji is one grapheme.",
            schema(
                json!({
                    "title": {"type": "string"},
                    "emoji": {"type": "string"},
                    "starts_at": {"type": "string"},
                    "place_name": {"type": "string"},
                    "kind": {"type": "string", "enum": ["cafe", "park", "hall", "square", "other_public", "not_public"]},
                    "lat": {"type": "number"},
                    "lon": {"type": "number"},
                    "capacity": {"type": "integer"},
                    "description": {"type": "string"},
                    "activity_tags": {"type": "array", "items": {"type": "string"}},
                    "women_only": {"type": "boolean"},
                }),
                &[
                    "title",
                    "emoji",
                    "starts_at",
                    "place_name",
                    "kind",
                    "lat",
                    "lon",
                ],
            ),
        ),
        spec(
            ToolName::JoinEvent,
            "Join an event after the user asked.",
            schema(json!({"event_id": {"type": "string"}}), &["event_id"]),
        ),
        spec(
            ToolName::CancelAttendance,
            "Cancel the user's attendance after they asked.",
            schema(json!({"event_id": {"type": "string"}}), &["event_id"]),
        ),
        spec(
            ToolName::CompleteAttendance,
            "Mark the user's attendance complete after they asked.",
            schema(json!({"event_id": {"type": "string"}}), &["event_id"]),
        ),
        spec(
            ToolName::SearchChatHistory,
            "Search this user's earlier messages.",
            schema(json!({"query": {"type": "string"}}), &["query"]),
        ),
    ])
}

fn spec(name: ToolName, description: &str, parameters: Value) -> ToolDefinition {
    ToolDefinition {
        name: name.as_str().to_string(),
        description: description.to_string(),
        parameters,
    }
}

fn schema(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

pub async fn execute<S: HarnessStore>(
    ctx: ToolContext<'_, S>,
    call: &ToolCall,
) -> Result<ToolResult, Error> {
    let Some(name) = ToolName::parse(&call.name) else {
        return Err(Error::BadArguments(format!("unknown tool {}", call.name)));
    };
    match name {
        ToolName::Remember => remember(&ctx, call).await,
        ToolName::Forget => forget(&ctx, call).await,
        ToolName::SetProfile => set_profile(&ctx, call).await,
        ToolName::SearchKnowledge => search_knowledge(&ctx, call).await,
        ToolName::SearchEvents => search_events(&ctx, call).await,
        ToolName::SearchPeople => search_people(&ctx, call).await,
        ToolName::ListMyEvents => list_my_events(&ctx, call).await,
        ToolName::CreateEvent => create_event(&ctx, call).await,
        ToolName::JoinEvent => join_event(&ctx, call).await,
        ToolName::CancelAttendance => cancel_attendance(&ctx, call).await,
        ToolName::CompleteAttendance => complete_attendance(&ctx, call).await,
        ToolName::SearchChatHistory => search_chat_history(&ctx, call).await,
    }
}

pub fn remember_args(call: &ToolCall) -> Result<NewMemory, Error> {
    let args: RememberArgs = parse(&call.arguments)?;
    Ok(NewMemory {
        durability: args.durability,
        key: args.key,
        value: args.value,
        quote: args.quote,
        confidence: args.confidence,
    })
}

pub fn profile_args(call: &ToolCall) -> Result<ProfilePatch, Error> {
    parse(&call.arguments)
}

pub fn event_args(call: &ToolCall) -> Result<NewEvent, Error> {
    let args: CreateEventArgs = parse(&call.arguments)?;
    let emoji = args.emoji.trim();
    if !single_grapheme(emoji) {
        return Err(Error::BadArguments(
            "emoji must be one grapheme".to_string(),
        ));
    }
    Ok(NewEvent {
        title: args.title,
        emoji: emoji.to_string(),
        description: args.description,
        starts_at: args.starts_at,
        capacity: args.capacity,
        activity_tags: args.activity_tags,
        women_only: args.women_only,
        place_name: args.place_name,
        place_kind: args.kind,
        latitude: args.lat,
        longitude: args.lon,
    })
}

fn parse<T: DeserializeOwned>(raw: &str) -> Result<T, Error> {
    let raw = if raw.trim().is_empty() { "{}" } else { raw };
    serde_json::from_str(raw).map_err(|_| Error::BadArguments("invalid arguments".to_string()))
}

fn failed(err: StoreError) -> Error {
    Error::Failed(err.to_string())
}

async fn remember<S: HarnessStore>(
    ctx: &ToolContext<'_, S>,
    call: &ToolCall,
) -> Result<ToolResult, Error> {
    let memory = remember_args(call)?;
    let saved = ctx
        .store
        .remember(ctx.user_id, memory)
        .await
        .map_err(failed)?;
    Ok(ToolResult::plain(memory_json(&saved)))
}

async fn forget<S: HarnessStore>(
    ctx: &ToolContext<'_, S>,
    call: &ToolCall,
) -> Result<ToolResult, Error> {
    let args: ForgetArgs = parse(&call.arguments)?;
    let removed = ctx
        .store
        .forget(ctx.user_id, &args.topic)
        .await
        .map_err(failed)?;
    Ok(ToolResult::plain(forget_json(&removed)))
}

async fn set_profile<S: HarnessStore>(
    ctx: &ToolContext<'_, S>,
    call: &ToolCall,
) -> Result<ToolResult, Error> {
    let patch = profile_args(call)?;
    let profile = ctx
        .store
        .apply_profile(ctx.user_id, &patch)
        .await
        .map_err(failed)?;
    Ok(ToolResult::plain(profile_json(&profile)))
}

async fn search_knowledge<S: HarnessStore>(
    ctx: &ToolContext<'_, S>,
    call: &ToolCall,
) -> Result<ToolResult, Error> {
    let args: SearchKnowledgeArgs = parse(&call.arguments)?;
    let query = Query {
        text: args.query,
        category: args.category,
    };
    let hits = ctx
        .store
        .search_knowledge(ctx.embedder, &query)
        .await
        .map_err(failed)?;
    let count = hits.len();
    Ok(ToolResult::hits(knowledge_json(&hits), count))
}

async fn search_events<S: HarnessStore>(
    ctx: &ToolContext<'_, S>,
    call: &ToolCall,
) -> Result<ToolResult, Error> {
    let args: SearchEventsArgs = parse(&call.arguments)?;
    let origin = origin(args.lat, args.lng)?;
    let profile = rank_profile(&ctx.store.profile(ctx.user_id).await.map_err(failed)?);
    let candidates = ctx
        .store
        .nearby_candidates(origin, rank::MAX_RADIUS_M, None, ctx.now)
        .await
        .map_err(failed)?;
    let want = embed_query(ctx.embedder, args.query.as_deref()).await;
    let ranked = rank::rank_events(&rank::RankInput {
        profile: &profile,
        origin,
        radius_m: rank::MAX_RADIUS_M,
        bounds: None,
        now: ctx.now,
        want_embedding: want.as_deref(),
        events: &candidates,
    })
    .map_err(|err| Error::Failed(err.to_string()))?;
    let count = ranked.len();
    Ok(ToolResult::hits(
        json!({
            "events": ranked.iter().map(scored_json).collect::<Vec<_>>(),
        }),
        count,
    ))
}

async fn search_people<S: HarnessStore>(
    ctx: &ToolContext<'_, S>,
    call: &ToolCall,
) -> Result<ToolResult, Error> {
    let args: SearchPeopleArgs = parse(&call.arguments)?;
    let location: PeopleLocation = parse(&call.arguments)?;
    let origin = origin(location.lat, location.lng)?;
    let profile = rank_profile(&ctx.store.profile(ctx.user_id).await.map_err(failed)?);
    let people = ctx
        .store
        .people(ctx.user_id, origin)
        .await
        .map_err(failed)?;
    let ranked = rank::rank_people(&rank::PeopleInput {
        profile: &profile,
        origin,
        now: ctx.now,
        people: &people,
    })
    .map_err(|err| Error::Failed(err.to_string()))?;
    let ranked = filter_people(ranked, args.query.as_deref());
    let count = ranked.len();
    Ok(ToolResult::hits(
        json!({
            "people": ranked.iter().map(person_json).collect::<Vec<_>>(),
        }),
        count,
    ))
}

async fn list_my_events<S: HarnessStore>(
    ctx: &ToolContext<'_, S>,
    call: &ToolCall,
) -> Result<ToolResult, Error> {
    let _: Value = parse(&call.arguments)?;
    let events = ctx
        .store
        .my_events(ctx.user_id, ctx.now)
        .await
        .map_err(failed)?;
    let count = events.len();
    Ok(ToolResult::hits(
        json!({
            "events": events.iter().map(event_json).collect::<Vec<_>>(),
        }),
        count,
    ))
}

async fn create_event<S: HarnessStore>(
    ctx: &ToolContext<'_, S>,
    call: &ToolCall,
) -> Result<ToolResult, Error> {
    let event = event_args(call)?;
    let saved = ctx
        .store
        .create_event(ctx.user_id, event)
        .await
        .map_err(failed)?;
    let id = saved.id;
    Ok(ToolResult::event(event_json(&saved), id))
}

async fn join_event<S: HarnessStore>(
    ctx: &ToolContext<'_, S>,
    call: &ToolCall,
) -> Result<ToolResult, Error> {
    let args: EventIdArgs = parse(&call.arguments)?;
    let attendance = ctx
        .store
        .join_event(args.event_id, ctx.user_id)
        .await
        .map_err(failed)?;
    Ok(ToolResult::event(
        attendance_json(&attendance),
        args.event_id,
    ))
}

async fn cancel_attendance<S: HarnessStore>(
    ctx: &ToolContext<'_, S>,
    call: &ToolCall,
) -> Result<ToolResult, Error> {
    let args: EventIdArgs = parse(&call.arguments)?;
    let attendance = ctx
        .store
        .cancel_attendance(args.event_id, ctx.user_id)
        .await
        .map_err(failed)?;
    Ok(ToolResult::event(
        attendance_json(&attendance),
        args.event_id,
    ))
}

async fn complete_attendance<S: HarnessStore>(
    ctx: &ToolContext<'_, S>,
    call: &ToolCall,
) -> Result<ToolResult, Error> {
    let args: EventIdArgs = parse(&call.arguments)?;
    let attendance = ctx
        .store
        .complete_attendance(args.event_id, ctx.user_id)
        .await
        .map_err(failed)?;
    Ok(ToolResult::event(
        attendance_json(&attendance),
        args.event_id,
    ))
}

async fn search_chat_history<S: HarnessStore>(
    ctx: &ToolContext<'_, S>,
    call: &ToolCall,
) -> Result<ToolResult, Error> {
    let args: SearchChatHistoryArgs = parse(&call.arguments)?;
    let messages = ctx
        .store
        .search_messages(ctx.user_id, &args.query)
        .await
        .map_err(failed)?;
    let count = messages.len();
    Ok(ToolResult::hits(
        json!({
            "messages": messages.iter().map(|message| json!({
                "id": message.id,
                "role": message.role,
                "body": message.body,
            })).collect::<Vec<_>>(),
        }),
        count,
    ))
}

fn origin(lat: Option<f64>, lng: Option<f64>) -> Result<rank::LatLng, Error> {
    match (lat, lng) {
        (Some(lat), Some(lng)) => Ok(rank::LatLng { lat, lng }),
        _ => Err(Error::BadArguments("lat and lng are required".to_string())),
    }
}

async fn embed_query(embedder: &dyn Embedder, query: Option<&str>) -> Option<Vec<f32>> {
    let query = query.map(str::trim).filter(|query| !query.is_empty())?;
    match embedder.embed(Input::Query, &[query.to_string()]).await {
        Ok(mut vectors) => vectors.pop(),
        Err(_) => None,
    }
}

fn rank_profile(profile: &Profile) -> rank::Profile {
    rank::Profile {
        age_band: profile.age_band.clone(),
        gender: profile.gender.clone(),
        mobility: profile.mobility.clone(),
        sportiness: profile.sportiness,
        likes: profile.likes.clone(),
        dislikes: profile.dislikes.clone(),
        women_only: profile.women_only,
        time_window: profile.time_window,
        embedding: profile.embedding.as_ref().map(|vector| vector.to_vec()),
    }
}

fn filter_people(mut hits: Vec<PersonHit>, query: Option<&str>) -> Vec<PersonHit> {
    let Some(query) = query.map(str::trim).filter(|query| !query.is_empty()) else {
        return hits;
    };
    let query = query.to_lowercase();
    hits.retain(|hit| {
        hit.first_name.to_lowercase().contains(&query)
            || hit
                .shared_tags
                .iter()
                .any(|tag| tag.to_lowercase().contains(&query))
    });
    hits
}

fn memory_json(memory: &Memory) -> Value {
    json!({
        "id": memory.id,
        "durability": memory.durability,
        "key": memory.key,
        "value": memory.value,
        "quote": memory.quote,
        "confidence": memory.confidence,
        "confirmed": memory.confirmed,
    })
}

fn forget_json(result: &ForgetResult) -> Value {
    if result.memories.is_empty() && result.tags.is_empty() {
        return json!({
            "matched": false,
            "message": "Nothing stored about that topic.",
        });
    }
    json!({
        "matched": true,
        "memories": result.memories.iter().map(memory_json).collect::<Vec<_>>(),
        "tags": result.tags,
    })
}

fn profile_json(profile: &Profile) -> Value {
    json!({
        "age_band": profile.age_band,
        "gender": profile.gender,
        "mobility": profile.mobility,
        "sportiness": profile.sportiness,
        "bio": profile.bio,
        "likes": profile.likes,
        "dislikes": profile.dislikes,
        "women_only": profile.women_only,
        "time_window": profile.time_window,
    })
}

fn knowledge_json(hits: &[Hit]) -> Value {
    json!({
        "hits": hits.iter().map(|hit| json!({
            "title": hit.title,
            "page_url": hit.page_url,
            "licence": hit.licence,
            "categories": hit.categories,
            "snippet": hit.snippet,
            "film_url": hit.film_url,
        })).collect::<Vec<_>>(),
    })
}

fn scored_json(event: &ScoredEvent) -> Value {
    json!({
        "id": event.id,
        "title": event.title,
        "emoji": event.emoji,
        "score": event.score,
        "distance_m": event.distance_m,
        "signed_count": event.signed_count,
        "capacity": event.capacity,
        "place_name": event.place_name,
        "latitude": event.latitude,
        "longitude": event.longitude,
        "starts_at": event.starts_at,
        "host_name": event.host_name,
        "promoted": event.promoted,
    })
}

fn person_json(hit: &PersonHit) -> Value {
    json!({
        "first_name": hit.first_name,
        "age_band": hit.age_band,
        "shared_tags": hit.shared_tags,
        "distance_band": hit.distance_band,
        "constraints_passed": hit.constraints_passed,
    })
}

fn event_json(event: &Event) -> Value {
    json!({
        "id": event.id,
        "title": event.title,
        "emoji": event.emoji,
        "description": event.description,
        "starts_at": event.starts_at,
        "capacity": event.capacity,
        "activity_tags": event.activity_tags,
        "status": event.status,
        "women_only": event.women_only,
        "place_name": event.place_name,
        "place_kind": event.place_kind,
        "latitude": event.latitude,
        "longitude": event.longitude,
        "signed_count": event.signed_count,
        "host_name": event.host_name,
    })
}

fn attendance_json(attendance: &Attendance) -> Value {
    json!({
        "event_id": attendance.event_id,
        "status": attendance.status,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::ToolCall;

    fn call(name: &str, arguments: &str) -> ToolCall {
        ToolCall {
            id: "call".into(),
            name: name.into(),
            arguments: arguments.into(),
        }
    }

    #[test]
    fn specs_name_every_tool() {
        let specs = specs().unwrap();
        let names: Vec<&str> = specs.iter().map(|tool| tool.name.as_str()).collect();
        let expected = [
            ToolName::Remember,
            ToolName::Forget,
            ToolName::SetProfile,
            ToolName::SearchKnowledge,
            ToolName::SearchEvents,
            ToolName::SearchPeople,
            ToolName::ListMyEvents,
            ToolName::CreateEvent,
            ToolName::JoinEvent,
            ToolName::CancelAttendance,
            ToolName::CompleteAttendance,
            ToolName::SearchChatHistory,
        ];
        assert_eq!(names.len(), expected.len());
        for name in expected {
            assert!(names.contains(&name.as_str()), "{}", name.as_str());
            assert!(ToolName::parse(name.as_str()).is_some());
        }
        assert!(ToolName::parse("drop_table").is_none());
    }

    #[test]
    fn remember_and_profile_args_parse_a_subset() {
        let memory = remember_args(&call(
            "remember",
            r#"{"durability":"long_term","key":"sport","value":"tennis","quote":"I play"}"#,
        ))
        .unwrap();
        assert_eq!(memory.key, "sport");
        assert_eq!(memory.value, "tennis");
        assert_eq!(memory.quote.as_deref(), Some("I play"));

        let patch = profile_args(&call(
            "set_profile",
            r#"{"locale":"en","like_tag":"chess"}"#,
        ))
        .unwrap();
        assert_eq!(patch.locale.as_deref(), Some("en"));
        assert_eq!(patch.like_tag.as_deref(), Some("chess"));
        assert!(patch.mobility.is_none());
        assert!(patch.display_name.is_none());
    }

    #[test]
    fn bad_json_is_rejected_before_a_value_exists() {
        let err = remember_args(&call("remember", "not-json")).unwrap_err();
        assert!(err.to_string().contains("invalid"));
        let err = profile_args(&call("set_profile", "{")).unwrap_err();
        assert!(err.to_string().contains("invalid"));
    }

    #[test]
    fn event_args_rejects_more_than_one_grapheme() {
        let err = event_args(&call("create_event", &event_json("ab"))).unwrap_err();
        assert!(err.to_string().contains("grapheme"), "{err}");
        assert!(event_args(&call("create_event", &event_json("🎾"))).is_ok());
    }

    #[test]
    fn single_grapheme_counts_one_emoji() {
        assert!(single_grapheme("🎾"));
        assert!(single_grapheme(" 👍 "));
        assert!(!single_grapheme("🎾🎾"));
        assert!(!single_grapheme("ab"));
        assert!(!single_grapheme("  "));
    }

    fn event_json(emoji: &str) -> String {
        format!(
            r#"{{"title":"Tennis","emoji":"{emoji}","starts_at":"2026-10-04T10:00:00Z","place_name":"Park","kind":"park","lat":52.2,"lon":21.0}}"#,
        )
    }
}
