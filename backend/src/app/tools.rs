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
            "Call this for a fact that is not a profile field, in the turn they say it. On this tool, loneliness and company count only as long_term key or value. quote and short_term do not count. A like tag with the same words is set_profile, not this tool. If they are lonely, live alone, or say samotna, samotny, or samotność, store long_term and keep lonely, lives alone, or samotn in value. If they want company or towarzystwo, store long_term and keep wants company or towarzystwo in value. short_term is only this plan, such as tomorrow at 11 or tired today. Do not use this for age, mobility, likes, dislikes, or language.",
            schema(
                json!({
                    "durability": {"type": "string", "enum": ["long_term", "short_term"], "description": "long_term stays. short_term is this plan only."},
                    "key": {"type": "string", "description": "Short label, such as company or plan."},
                    "value": {"type": "string", "description": "The fact, keeping lonely, lives alone, samotn, wants company, or towarzystwo when those apply."},
                    "quote": {"type": "string", "description": "Their words, when you have them."},
                    "confidence": {"type": "number", "description": "From 0 to 1. Omit if unsure."},
                }),
                &["durability", "key", "value"],
            ),
        ),
        spec(
            ToolName::Forget,
            "Call this when they say a stored fact is wrong or they want it gone. topic is their words, not an id. If matched is false, say nothing was stored about that.",
            schema(
                json!({"topic": {"type": "string", "description": "Their words for the fact to drop."}}),
                &["topic"],
            ),
        ),
        spec(
            ToolName::SetProfile,
            "Call this to store profile fields they just stated. Send only those fields. One like_tag or dislike_tag per call. mobility must include the separate word wheelchair when they use one, or the tags padel, tennis, basketball, volleyball, squash, badminton, football, soccer, court, running, and run are not dropped. In Polish say na wózku. In English say they use a wheelchair. age_band is one of 18-24, 25-34, 35-44, 45-54, 55-64, 65+, the band that contains the age they gave. Ask if they were vague. locale is pl or en, only when they ask to switch. sportiness is 0 to 3 only if they said how active they are. Match tags are English singular words: walk, coffee, padel, tennis, basketball, volleyball, squash, badminton, football, running. A like_tag that contains lonely, lives alone, samotn, wants company, or towarzystwo also counts as company fit.",
            schema(
                json!({
                    "display_name": {"type": "string", "description": "Name they asked to be called."},
                    "locale": {"type": "string", "description": "pl or en, only when they ask to switch language."},
                    "age_band": {"type": "string", "description": "One of 18-24, 25-34, 35-44, 45-54, 55-64, 65+."},
                    "gender": {"type": "string", "description": "Only if they stated it."},
                    "mobility": {"type": "string", "description": "Must contain wheelchair if they use one."},
                    "sportiness": {"type": "integer", "description": "0 almost no sport, 3 very active. Omit if they did not say."},
                    "women_only": {"type": "boolean", "description": "True only when they want to meet women only."},
                    "time_window": {
                        "type": "object",
                        "description": "Standing hours they can go out, Europe/Warsaw local. Not a one-off such as tomorrow at 11. That is a short_term memory.",
                        "properties": {
                            "start_minute": {"type": "integer", "description": "Minutes from local midnight, inclusive."},
                            "end_minute": {"type": "integer", "description": "Minutes from local midnight, exclusive."},
                        },
                        "required": ["start_minute", "end_minute"],
                    },
                    "like_tag": {"type": "string", "description": "One tag. Use walk, coffee, or a sport word from the description."},
                    "dislike_tag": {"type": "string", "description": "One tag they refuse. Use padel, not paddle."},
                    "bio": {"type": "string", "description": "Only a short bio they asked to store."},
                }),
                &[],
            ),
        ),
        spec(
            ToolName::SearchKnowledge,
            "Call this when they ask about a service, a problem, loneliness, ageing, disability, or an existing innovation. query is their problem in their words, not a project name you guessed. An empty hits list means the library has nothing. Say that. Cite only title and page_url from hits. Add film_url only if they ask for a film. Do not offer to turn a hit into a grant, a form, or a service for a gmina.",
            schema(
                json!({
                    "query": {"type": "string", "description": "Their problem or the name they actually said."},
                    "category": {"type": "string", "description": "Optional. Only a category they named."},
                }),
                &["query"],
            ),
        ),
        spec(
            ToolName::SearchEvents,
            "Call this when they want somewhere to go. lat and lng are required. Use coordinates they gave or from an earlier tool result. If you have none, use lat 50.0683 and lng 19.9917, TAURON Arena at ul. Stanisława Lema 7, and say you looked around the arena. query is optional. Do not add an activity the filters dropped. If capacity is set and signed_count is at least capacity, it is full: do not offer it, and join will fail. Speak one event unless they asked for a list: title, place_name, starts_at in local words, and host_name as stored. If promoted is true, say it is promoted. Do not speak score, distance_m, or coordinates. An empty events list means nothing in range passed.",
            schema(
                json!({
                    "query": {"type": "string", "description": "Optional. The activity in a few words, such as walk or cafe."},
                    "lat": {"type": "number", "description": "Latitude. 50.0683 only as the arena fallback."},
                    "lng": {"type": "number", "description": "Longitude. 19.9917 only as the arena fallback."},
                }),
                &["lat", "lng"],
            ),
        ),
        spec(
            ToolName::SearchPeople,
            "Call this when they want company. lat and lng follow the same rule as search_events. There is no radius cap, so a farther person can still be returned. query is optional and only filters the stored name or shared tags. first_name is the display name, not a parsed given name. Say that name, the age band, shared interests, and the distance: within_500m as a short walk, within_2km as nearby, within_5km as a bit further, farther as a longer way. Say constraints in words. Do not invent a phone, an address, or a health note. An empty list means nobody with a live public meetup passed the filters.",
            schema(
                json!({
                    "query": {"type": "string", "description": "Optional. A first name or a shared interest they named."},
                    "lat": {"type": "number", "description": "Latitude. 50.0683 only as the arena fallback."},
                    "lng": {"type": "number", "description": "Longitude. 19.9917 only as the arena fallback."},
                }),
                &["lat", "lng"],
            ),
        ),
        spec(
            ToolName::ListMyEvents,
            "Call this when they ask which meetups they are going to. Pass no arguments. It returns events where their attendance is going, not an event they only host. The status field is the event status, usually scheduled, not the attendance. If they just created one, speak from that create_event result. Speak title, place_name, and starts_at in local words.",
            schema(json!({}), &[]),
        ),
        spec(
            ToolName::CreateEvent,
            "Call this only after they asked you to create this meetup. The longitude field is lon, not lng. Copy a search result's longitude into lon. Do not invent coordinates, and do not reuse the arena fallback unless they are meeting at the arena. starts_at is RFC3339 UTC. Europe/Warsaw is UTC+2 from 01:00 UTC on the last Sunday of March until 01:00 UTC on the last Sunday of October, otherwise UTC+1. place_name must be public. The store rejects kind not_public only, so do not send a home under a public kind. emoji is one grapheme and is not spoken. women_only true only when they asked for a women-only meetup. activity_tags use the same English words as set_profile. Omit capacity and description unless they gave them. Hosting does not mark them as going.",
            schema(
                json!({
                    "title": {"type": "string", "description": "Short name of the meetup."},
                    "emoji": {"type": "string", "description": "One grapheme. Not spoken in the reply."},
                    "starts_at": {"type": "string", "description": "RFC3339 UTC, converted from Europe/Warsaw."},
                    "place_name": {"type": "string", "description": "Public place name. Never a home."},
                    "kind": {"type": "string", "enum": ["cafe", "park", "hall", "square", "other_public"], "description": "Public kind. Never a home."},
                    "lat": {"type": "number", "description": "From the person or a tool result for this place."},
                    "lon": {"type": "number", "description": "From the person or a tool result for this place."},
                    "capacity": {"type": "integer", "description": "Only if they gave a number."},
                    "description": {"type": "string", "description": "Only if they gave one."},
                    "activity_tags": {"type": "array", "items": {"type": "string"}, "description": "English match words, such as walk or coffee."},
                    "women_only": {"type": "boolean", "description": "True only when they asked for women only."},
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
            "Call this only after they asked to join a specific event. event_id must be an id from search_events or list_my_events in this conversation. Success is status going. If the result has error, including event is full, the join did not happen. Say that in a sentence.",
            schema(
                json!({"event_id": {"type": "string", "description": "id from search_events or list_my_events."}}),
                &["event_id"],
            ),
        ),
        spec(
            ToolName::CancelAttendance,
            "Call this only after they asked to cancel their place. event_id comes from list_my_events or search_events. If the result has error, it was not cancelled. Say that.",
            schema(
                json!({"event_id": {"type": "string", "description": "id from list_my_events or search_events."}}),
                &["event_id"],
            ),
        ),
        spec(
            ToolName::CompleteAttendance,
            "Call this only after they said they went, or asked to mark it done. event_id comes from their events. If the result has error, it was not marked done. Say that.",
            schema(
                json!({"event_id": {"type": "string", "description": "id from list_my_events."}}),
                &["event_id"],
            ),
        ),
        spec(
            ToolName::SearchChatHistory,
            "Call this only when they refer to something said earlier that is not in the recent turns or the summary. query is their words. Do not use it for innovations or events.",
            schema(
                json!({"query": {"type": "string", "description": "Words from the earlier conversation."}}),
                &["query"],
            ),
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
        .nearby_candidates(ctx.user_id, origin, rank::MAX_RADIUS_M, None, ctx.now)
        .await
        .map_err(failed)?;
    let query_vector = embed_query(ctx.embedder, args.query.as_deref()).await;
    let want = event_want(query_vector, profile.embedding.as_deref());
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

/// A search string is the want. With no string, the stored profile vector is the want.
pub(crate) fn event_want(query: Option<Vec<f32>>, profile: Option<&[f32]>) -> Option<Vec<f32>> {
    if let Some(query) = query {
        return Some(query);
    }
    profile.map(|vector| vector.to_vec())
}

pub(crate) fn rank_profile(profile: &Profile) -> rank::Profile {
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
    fn specs_match_the_filters_the_ranker_actually_runs() {
        let specs = specs().unwrap();
        let by_name = |name: &str| specs.iter().find(|tool| tool.name == name).expect(name);
        assert!(by_name("remember").description.contains("long_term"));
        assert!(by_name("remember").description.contains("samotn"));
        assert!(by_name("remember").description.contains("towarzystwo"));
        assert!(by_name("set_profile").description.contains("65+"));
        assert!(by_name("list_my_events").description.contains("going"));
        assert!(by_name("create_event").description.contains("lon, not lng"));
        assert!(by_name("set_profile").description.contains("wheelchair"));
        assert!(by_name("search_events").description.contains("50.0683"));
        assert!(by_name("search_events").description.contains("19.9917"));
        assert!(by_name("search_knowledge").description.contains("page_url"));
        let create = by_name("create_event");
        let kinds: Vec<&str> = create.parameters["properties"]["kind"]["enum"]
            .as_array()
            .expect("kind enum")
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert!(!kinds.contains(&"not_public"), "{kinds:?}");
        assert!(create.description.contains("RFC3339"));
    }

    #[test]
    fn event_want_uses_the_query_vector_then_the_profile_vector() {
        let query = vec![1.0, 0.0];
        let profile = vec![0.0, 1.0];
        assert_eq!(
            event_want(Some(query.clone()), Some(&profile)).as_deref(),
            Some(query.as_slice())
        );
        assert_eq!(
            event_want(None, Some(&profile)).as_deref(),
            Some(profile.as_slice())
        );
        assert_eq!(event_want(None, None), None);
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
