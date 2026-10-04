use std::path::Path;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::appdb::{
    Attendance, ChatMessage, ChatRole, Conversation, Event, ForgetResult, Memory, NewEvent,
    NewMemory, Profile, ProfilePatch, Store, Turn, User,
};
use crate::config::Config;
use crate::embed::Embedder;
use crate::knowledge::{self, Hit, Query};
use crate::llm::{
    AssistantMessage, CompletionRequest, Message, Model, StreamItem, ToolCall, ToolChoice,
    ToolDefinition,
};
use crate::prompt;
use crate::rank::{self, Candidate, Person};
use crate::speech::Speech;
use crate::sse::{
    AudioReady, Kind, ReplyDelta, ReplyDone, ToolFinished, ToolStarted, TranscriptReady,
    TurnFailed, TurnRef,
};
use crate::tools::{self, ToolContext};

pub const MAX_ROUNDS: u8 = 8;
pub const RECENT_USER_TURNS: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Store(String),
    Model(String),
    Speech(String),
    Prompt(String),
    MissingTurn,
    MissingTranscript,
    Checkpoint,
    Audio,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.public_message())
    }
}

impl std::error::Error for Error {}

impl Error {
    fn public_message(&self) -> String {
        match self {
            Self::Model(_) => "chat completion failed".to_string(),
            Self::Speech(message) => format!("speech synthesis failed: {message}"),
            Self::Audio => "audio failed".to_string(),
            Self::Checkpoint => "checkpoint failed".to_string(),
            Self::MissingTranscript => "missing transcript".to_string(),
            Self::MissingTurn => "turn not found".to_string(),
            Self::Store(message) => format!("store failed: {message}"),
            Self::Prompt(message) => format!("prompt failed: {message}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreError {
    message: String,
}

impl StoreError {
    pub fn new(err: impl std::fmt::Display) -> Self {
        Self {
            message: err.to_string(),
        }
    }
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for StoreError {}

/// Store methods the turn loop and tools call.
///
/// `messages_before` is the newest `limit` rows for that user, strictly before
/// `before` when it is set. The page itself is oldest-first. `before == None`
/// is the newest page.
#[async_trait]
pub trait HarnessStore: Send + Sync {
    async fn running_turns(&self) -> Result<Vec<Turn>, StoreError>;
    async fn user(&self, id: Uuid) -> Result<User, StoreError>;
    async fn profile(&self, user_id: Uuid) -> Result<Profile, StoreError>;
    async fn memories(&self, user_id: Uuid) -> Result<Vec<Memory>, StoreError>;
    async fn conversation(&self, user_id: Uuid) -> Result<Conversation, StoreError>;
    async fn recent_turns(
        &self,
        user_id: Uuid,
        user_turns: i64,
    ) -> Result<Vec<ChatMessage>, StoreError>;
    async fn messages_before(
        &self,
        user_id: Uuid,
        before: Option<Uuid>,
        limit: i64,
    ) -> Result<Vec<ChatMessage>, StoreError>;
    async fn count_user_messages(&self, user_id: Uuid) -> Result<i64, StoreError>;
    async fn insert_message(
        &self,
        user_id: Uuid,
        role: ChatRole,
        body: &str,
    ) -> Result<ChatMessage, StoreError>;
    async fn set_checkpoint(&self, turn_id: Uuid, checkpoint: Value) -> Result<(), StoreError>;
    async fn finish_turn(
        &self,
        turn_id: Uuid,
        reply_text: &str,
        audio_path: Option<&str>,
    ) -> Result<(), StoreError>;
    async fn fail_turn(&self, turn_id: Uuid, message: &str) -> Result<(), StoreError>;
    async fn append_stream(
        &self,
        user_id: Uuid,
        turn_id: Uuid,
        kind: &str,
        payload: Value,
    ) -> Result<i64, StoreError>;
    async fn set_summary(
        &self,
        user_id: Uuid,
        summary: &str,
        through_message: Uuid,
    ) -> Result<(), StoreError>;
    async fn remember(&self, user_id: Uuid, memory: NewMemory) -> Result<Memory, StoreError>;
    async fn forget(&self, user_id: Uuid, topic: &str) -> Result<ForgetResult, StoreError>;
    async fn apply_profile(
        &self,
        user_id: Uuid,
        patch: &ProfilePatch,
    ) -> Result<Profile, StoreError>;
    async fn create_event(&self, host_id: Uuid, event: NewEvent) -> Result<Event, StoreError>;
    async fn join_event(&self, event_id: Uuid, user_id: Uuid) -> Result<Attendance, StoreError>;
    async fn cancel_attendance(
        &self,
        event_id: Uuid,
        user_id: Uuid,
    ) -> Result<Attendance, StoreError>;
    async fn complete_attendance(
        &self,
        event_id: Uuid,
        user_id: Uuid,
    ) -> Result<Attendance, StoreError>;
    async fn nearby_candidates(
        &self,
        viewer: Uuid,
        origin: rank::LatLng,
        radius_m: f64,
        bounds: Option<rank::BBox>,
        now: DateTime<Utc>,
    ) -> Result<Vec<Candidate>, StoreError>;
    async fn my_events(&self, user_id: Uuid, now: DateTime<Utc>) -> Result<Vec<Event>, StoreError>;
    async fn people(&self, viewer: Uuid, origin: rank::LatLng) -> Result<Vec<Person>, StoreError>;
    async fn search_messages(
        &self,
        user_id: Uuid,
        query: &str,
    ) -> Result<Vec<ChatMessage>, StoreError>;
    async fn search_knowledge(
        &self,
        embedder: &dyn Embedder,
        query: &Query,
    ) -> Result<Vec<Hit>, StoreError>;
}

#[async_trait]
impl HarnessStore for Store {
    async fn running_turns(&self) -> Result<Vec<Turn>, StoreError> {
        Store::running_turns(self).await.map_err(StoreError::new)
    }

    async fn user(&self, id: Uuid) -> Result<User, StoreError> {
        Store::user(self, id).await.map_err(StoreError::new)
    }

    async fn profile(&self, user_id: Uuid) -> Result<Profile, StoreError> {
        Store::profile(self, user_id).await.map_err(StoreError::new)
    }

    async fn memories(&self, user_id: Uuid) -> Result<Vec<Memory>, StoreError> {
        Store::memories(self, user_id)
            .await
            .map_err(StoreError::new)
    }

    async fn conversation(&self, user_id: Uuid) -> Result<Conversation, StoreError> {
        Store::conversation(self, user_id)
            .await
            .map_err(StoreError::new)
    }

    async fn recent_turns(
        &self,
        user_id: Uuid,
        user_turns: i64,
    ) -> Result<Vec<ChatMessage>, StoreError> {
        Store::recent_turns(self, user_id, user_turns)
            .await
            .map_err(StoreError::new)
    }

    async fn messages_before(
        &self,
        user_id: Uuid,
        before: Option<Uuid>,
        limit: i64,
    ) -> Result<Vec<ChatMessage>, StoreError> {
        Store::messages_before(self, user_id, before, limit)
            .await
            .map_err(StoreError::new)
    }

    async fn count_user_messages(&self, user_id: Uuid) -> Result<i64, StoreError> {
        Store::count_user_messages(self, user_id)
            .await
            .map_err(StoreError::new)
    }

    async fn insert_message(
        &self,
        user_id: Uuid,
        role: ChatRole,
        body: &str,
    ) -> Result<ChatMessage, StoreError> {
        Store::insert_message(self, user_id, role, body)
            .await
            .map_err(StoreError::new)
    }

    async fn set_checkpoint(&self, turn_id: Uuid, checkpoint: Value) -> Result<(), StoreError> {
        Store::set_checkpoint(self, turn_id, checkpoint)
            .await
            .map_err(StoreError::new)
    }

    async fn finish_turn(
        &self,
        turn_id: Uuid,
        reply_text: &str,
        audio_path: Option<&str>,
    ) -> Result<(), StoreError> {
        Store::finish_turn(self, turn_id, reply_text, audio_path)
            .await
            .map_err(StoreError::new)
    }

    async fn fail_turn(&self, turn_id: Uuid, message: &str) -> Result<(), StoreError> {
        Store::fail_turn(self, turn_id, message)
            .await
            .map_err(StoreError::new)
    }

    async fn append_stream(
        &self,
        user_id: Uuid,
        turn_id: Uuid,
        kind: &str,
        payload: Value,
    ) -> Result<i64, StoreError> {
        Store::append_stream(self, user_id, turn_id, kind, payload)
            .await
            .map_err(StoreError::new)
    }

    async fn set_summary(
        &self,
        user_id: Uuid,
        summary: &str,
        through_message: Uuid,
    ) -> Result<(), StoreError> {
        Store::set_summary(self, user_id, summary, through_message)
            .await
            .map_err(StoreError::new)
    }

    async fn remember(&self, user_id: Uuid, memory: NewMemory) -> Result<Memory, StoreError> {
        Store::remember(self, user_id, memory)
            .await
            .map_err(StoreError::new)
    }

    async fn forget(&self, user_id: Uuid, topic: &str) -> Result<ForgetResult, StoreError> {
        Store::forget(self, user_id, topic)
            .await
            .map_err(StoreError::new)
    }

    async fn apply_profile(
        &self,
        user_id: Uuid,
        patch: &ProfilePatch,
    ) -> Result<Profile, StoreError> {
        Store::apply_profile(self, user_id, patch)
            .await
            .map_err(StoreError::new)
    }

    async fn create_event(&self, host_id: Uuid, event: NewEvent) -> Result<Event, StoreError> {
        Store::create_event(self, host_id, event)
            .await
            .map_err(StoreError::new)
    }

    async fn join_event(&self, event_id: Uuid, user_id: Uuid) -> Result<Attendance, StoreError> {
        Store::join_event(self, event_id, user_id)
            .await
            .map_err(StoreError::new)
    }

    async fn cancel_attendance(
        &self,
        event_id: Uuid,
        user_id: Uuid,
    ) -> Result<Attendance, StoreError> {
        Store::cancel_attendance(self, event_id, user_id)
            .await
            .map_err(StoreError::new)
    }

    async fn complete_attendance(
        &self,
        event_id: Uuid,
        user_id: Uuid,
    ) -> Result<Attendance, StoreError> {
        Store::complete_attendance(self, event_id, user_id)
            .await
            .map_err(StoreError::new)
    }

    async fn nearby_candidates(
        &self,
        viewer: Uuid,
        origin: rank::LatLng,
        radius_m: f64,
        bounds: Option<rank::BBox>,
        now: DateTime<Utc>,
    ) -> Result<Vec<Candidate>, StoreError> {
        Store::nearby_candidates(self, viewer, origin, radius_m, bounds, now)
            .await
            .map_err(StoreError::new)
    }

    async fn my_events(&self, user_id: Uuid, now: DateTime<Utc>) -> Result<Vec<Event>, StoreError> {
        Store::my_events(self, user_id, now)
            .await
            .map_err(StoreError::new)
    }

    async fn people(&self, viewer: Uuid, origin: rank::LatLng) -> Result<Vec<Person>, StoreError> {
        Store::people(self, viewer, origin)
            .await
            .map_err(StoreError::new)
    }

    async fn search_messages(
        &self,
        user_id: Uuid,
        query: &str,
    ) -> Result<Vec<ChatMessage>, StoreError> {
        Store::search_messages(self, user_id, query)
            .await
            .map_err(StoreError::new)
    }

    async fn search_knowledge(
        &self,
        embedder: &dyn Embedder,
        query: &Query,
    ) -> Result<Vec<Hit>, StoreError> {
        knowledge::search(self.pool(), embedder, query)
            .await
            .map_err(StoreError::new)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Checkpoint {
    pub round: u32,
    pub messages: Vec<Message>,
    pub executed_tool_ids: Vec<String>,
}

pub struct Services<'a, S = Store> {
    pub store: &'a S,
    pub model: &'a dyn Model,
    pub speech: &'a dyn Speech,
    pub embedder: &'a dyn Embedder,
    pub config: &'a Config,
}

impl<S> Copy for Services<'_, S> {}

impl<S> Clone for Services<'_, S> {
    fn clone(&self) -> Self {
        *self
    }
}

pub async fn run_turn<S: HarnessStore>(
    services: Services<'_, S>,
    turn_id: Uuid,
) -> Result<(), Error> {
    if let Err(err) = drive(services, turn_id).await {
        tracing::warn!(%turn_id, error = %err, "turn failed");
        if let Ok(turns) = services.store.running_turns().await {
            if let Some(turn) = turns.into_iter().find(|turn| turn.id == turn_id) {
                let message = err.public_message();
                let _ = services.store.fail_turn(turn_id, &message).await;
                let _ = emit(
                    services.store,
                    turn.user_id,
                    turn_id,
                    Kind::TurnFailed,
                    &TurnFailed {
                        turn_id,
                        error: message,
                    },
                )
                .await;
            }
        }
        return Err(err);
    }
    Ok(())
}

pub async fn resume_running<S: HarnessStore>(services: Services<'_, S>) -> Result<(), Error> {
    let turns = services.store.running_turns().await.map_err(store_err)?;
    let mut first = None;
    for turn in turns {
        if let Err(err) = run_turn(services, turn.id).await {
            tracing::error!(turn_id = %turn.id, error = %err, "resume turn");
            if first.is_none() {
                first = Some(err);
            }
        }
    }
    match first {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

async fn drive<S: HarnessStore>(services: Services<'_, S>, turn_id: Uuid) -> Result<(), Error> {
    let store = services.store;
    let turn = load_turn(store, turn_id).await?;
    let user_id = turn.user_id;
    let user = store.user(user_id).await.map_err(store_err)?;
    let locale = user.locale;
    tracing::info!(%turn_id, %user_id, "turn");
    let mut checkpoint = match decode_checkpoint(&turn)? {
        Some(checkpoint) => checkpoint,
        None => fresh(&services, &turn, &locale).await?,
    };
    let tool_specs = tools::specs().map_err(|err| Error::Prompt(err.to_string()))?;
    let mut reply = None;
    let mut from_stream = false;
    loop {
        if checkpoint.round >= u32::from(MAX_ROUNDS) {
            break;
        }
        checkpoint.round += 1;
        let assistant = stream_round(
            &services,
            user_id,
            turn_id,
            &checkpoint.messages,
            &tool_specs,
        )
        .await?;
        let calls = assistant.tool_calls.clone();
        if calls.is_empty() {
            let text = assistant.content.clone().unwrap_or_default();
            if !text.trim().is_empty() {
                reply = Some(text);
                from_stream = true;
            }
            break;
        }
        if calls
            .iter()
            .all(|call| already_executed(&checkpoint, &call.id))
        {
            save_checkpoint(store, turn_id, &checkpoint).await?;
            continue;
        }
        checkpoint.messages.push(Message::Assistant {
            content: assistant.content.clone(),
            tool_calls: calls.clone(),
        });
        for call in &calls {
            if already_executed(&checkpoint, &call.id) {
                checkpoint
                    .messages
                    .push(tool_message(call, &json!({"error": "already completed"})));
                continue;
            }
            let (tool_name, label) = tool_label(&call.name);
            tracing::info!(%turn_id, tool = %tool_name, "tool");
            emit(
                store,
                user_id,
                turn_id,
                Kind::ToolStarted,
                &ToolStarted {
                    turn_id,
                    tool: tool_name.clone(),
                    label,
                },
            )
            .await?;
            let outcome = tools::execute(
                ToolContext {
                    store,
                    embedder: services.embedder,
                    user_id,
                    now: Utc::now(),
                },
                call,
            )
            .await;
            let (body, ok, event_id, hit_count) = match outcome {
                Ok(result) => (result.body, true, result.event_id, result.hit_count),
                Err(err) => (json!({"error": err.to_string()}), false, None, None),
            };
            emit(
                store,
                user_id,
                turn_id,
                Kind::ToolFinished,
                &ToolFinished {
                    turn_id,
                    tool: tool_name,
                    ok,
                    event_id,
                    hit_count,
                },
            )
            .await?;
            checkpoint.messages.push(tool_message(call, &body));
            checkpoint.executed_tool_ids.push(call.id.clone());
            save_checkpoint(store, turn_id, &checkpoint).await?;
        }
    }

    let reply = match reply {
        Some(text) => text,
        None => {
            let repaired = repair(&services, &checkpoint.messages, &locale).await?;
            if repaired.trim().is_empty() {
                apology(&locale).to_string()
            } else {
                repaired
            }
        }
    };
    if !from_stream {
        emit(
            store,
            user_id,
            turn_id,
            Kind::ReplyDelta,
            &ReplyDelta {
                turn_id,
                text: reply.clone(),
            },
        )
        .await?;
    }
    store
        .insert_message(user_id, ChatRole::Assistant, &reply)
        .await
        .map_err(store_err)?;
    emit(
        store,
        user_id,
        turn_id,
        Kind::ReplyDone,
        &ReplyDone {
            turn_id,
            text: reply.clone(),
        },
    )
    .await?;
    let audio = services
        .speech
        .speak(&reply)
        .await
        .map_err(|err| Error::Speech(err.to_string()))?;
    let audio_path = write_audio(&services.config.audio_dir, turn_id, audio.as_ref()).await?;
    store
        .finish_turn(turn_id, &reply, Some(&audio_path))
        .await
        .map_err(store_err)?;
    emit(
        store,
        user_id,
        turn_id,
        Kind::AudioReady,
        &AudioReady {
            turn_id,
            url: format!("/v1/chat/turns/{turn_id}/audio"),
        },
    )
    .await?;
    emit(
        store,
        user_id,
        turn_id,
        Kind::TurnDone,
        &TurnRef { turn_id },
    )
    .await?;
    compact(&services, user_id, &locale).await?;
    Ok(())
}

async fn fresh<S: HarnessStore>(
    services: &Services<'_, S>,
    turn: &Turn,
    locale: &str,
) -> Result<Checkpoint, Error> {
    let user_text = turn
        .user_text
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .ok_or(Error::MissingTranscript)?
        .to_string();
    let store = services.store;
    let user_id = turn.user_id;
    emit(
        store,
        user_id,
        turn.id,
        Kind::TurnStarted,
        &TurnRef { turn_id: turn.id },
    )
    .await?;
    emit(
        store,
        user_id,
        turn.id,
        Kind::TranscriptReady,
        &TranscriptReady {
            turn_id: turn.id,
            text: user_text.clone(),
        },
    )
    .await?;
    let recent = store
        .recent_turns(user_id, RECENT_USER_TURNS as i64)
        .await
        .map_err(store_err)?;
    store
        .insert_message(user_id, ChatRole::User, &user_text)
        .await
        .map_err(store_err)?;
    let profile = store.profile(user_id).await.map_err(store_err)?;
    let memories = store.memories(user_id).await.map_err(store_err)?;
    let conversation = store.conversation(user_id).await.map_err(store_err)?;
    let messages = prompt::messages(
        locale,
        &profile,
        &memories,
        conversation.summary.as_deref(),
        &recent,
        &user_text,
    );
    let checkpoint = Checkpoint {
        round: 0,
        messages,
        executed_tool_ids: Vec::new(),
    };
    save_checkpoint(store, turn.id, &checkpoint).await?;
    Ok(checkpoint)
}

async fn stream_round<S: HarnessStore>(
    services: &Services<'_, S>,
    user_id: Uuid,
    turn_id: Uuid,
    messages: &[Message],
    tool_specs: &[ToolDefinition],
) -> Result<AssistantMessage, Error> {
    let mut request = CompletionRequest::new(messages.to_vec());
    request.tools = tool_specs.to_vec();
    let mut stream = services
        .model
        .stream(&request)
        .await
        .map_err(|err| Error::Model(err.to_string()))?;
    let mut assembled = String::new();
    let mut finished = None;
    while let Some(item) = stream.next().await {
        match item.map_err(|err| Error::Model(err.to_string()))? {
            StreamItem::TextDelta(text) => {
                if text.is_empty() {
                    continue;
                }
                assembled.push_str(&text);
                emit(
                    services.store,
                    user_id,
                    turn_id,
                    Kind::ReplyDelta,
                    &ReplyDelta { turn_id, text },
                )
                .await?;
            }
            StreamItem::Done(message) => finished = Some(message),
        }
    }
    Ok(finished.unwrap_or(AssistantMessage {
        content: if assembled.is_empty() {
            None
        } else {
            Some(assembled)
        },
        tool_calls: Vec::new(),
    }))
}

async fn repair<S: HarnessStore>(
    services: &Services<'_, S>,
    messages: &[Message],
    locale: &str,
) -> Result<String, Error> {
    let mut messages = messages.to_vec();
    messages.push(Message::User {
        content: if prompt::english_locale(locale) {
            "The previous reply was empty. Answer in plain text without tools. If you have no event and no library result, say so. Do not invent a URL or an event.".into()
        } else {
            "Poprzednia odpowiedź była pusta. Odpowiedz zwykłym tekstem, bez narzędzi. Jeśli nie masz wydarzenia ani wyniku z biblioteki, powiedz to. Nie wymyślaj adresu URL ani wydarzenia.".into()
        },
    });
    let mut request = CompletionRequest::new(messages);
    request.tool_choice = ToolChoice::None;
    request.max_tokens = Some(400);
    let message = services
        .model
        .complete(&request)
        .await
        .map_err(|err| Error::Model(err.to_string()))?;
    Ok(message.content.unwrap_or_default())
}

async fn compact<S: HarnessStore>(
    services: &Services<'_, S>,
    user_id: Uuid,
    locale: &str,
) -> Result<(), Error> {
    let count = services
        .store
        .count_user_messages(user_id)
        .await
        .map_err(store_err)?;
    if count <= 0 || count % 3 != 0 {
        return Ok(());
    }
    let history = history(services.store, user_id).await?;
    let (older, through) = older_than_latest(&history);
    let mut messages = vec![Message::System {
        content: summary_instruction(locale).to_string(),
    }];
    for message in &older {
        messages.push(match message.role {
            ChatRole::User => Message::User {
                content: message.body.clone(),
            },
            ChatRole::Assistant => Message::Assistant {
                content: Some(message.body.clone()),
                tool_calls: Vec::new(),
            },
        });
    }
    let mut request = CompletionRequest::new(messages);
    request.tool_choice = ToolChoice::None;
    request.max_tokens = Some(400);
    let summary = services
        .model
        .complete(&request)
        .await
        .map_err(|err| Error::Model(err.to_string()))?;
    if let (Some(text), Some(through)) = (
        summary
            .content
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty()),
        through,
    ) {
        services
            .store
            .set_summary(user_id, text, through)
            .await
            .map_err(store_err)?;
    }
    Ok(())
}

async fn history<S: HarnessStore>(store: &S, user_id: Uuid) -> Result<Vec<ChatMessage>, Error> {
    let mut pages = Vec::new();
    let mut before = None;
    loop {
        let page = store
            .messages_before(user_id, before, 100)
            .await
            .map_err(store_err)?;
        if page.is_empty() {
            break;
        }
        let oldest = page[0].id;
        if before == Some(oldest) {
            break;
        }
        let done = page.len() < 100;
        before = Some(oldest);
        pages.push(page);
        if done {
            break;
        }
    }
    let mut chronological = Vec::new();
    for page in pages.into_iter().rev() {
        chronological.extend(page);
    }
    Ok(chronological)
}

fn older_than_latest(messages: &[ChatMessage]) -> (Vec<ChatMessage>, Option<Uuid>) {
    let user_at: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter(|(_, message)| message.role == ChatRole::User)
        .map(|(index, _)| index)
        .collect();
    if user_at.len() <= RECENT_USER_TURNS {
        return (Vec::new(), None);
    }
    let cut = user_at[user_at.len() - RECENT_USER_TURNS];
    let older = messages[..cut].to_vec();
    let through = older.last().map(|message| message.id);
    (older, through)
}

fn apology(locale: &str) -> &'static str {
    if prompt::english_locale(locale) {
        "Sorry, I could not answer. I am not inventing an event or a citation."
    } else {
        "Przepraszam, nie potrafię teraz odpowiedzieć. Nie wymyślam wydarzenia ani źródła."
    }
}

fn summary_instruction(locale: &str) -> &'static str {
    if prompt::english_locale(locale) {
        "Summarize the older conversation in a few sentences. Do not invent events or citations."
    } else {
        "Streść starszą rozmowę w kilku zdaniach. Nie wymyślaj wydarzeń ani źródeł."
    }
}

fn already_executed(checkpoint: &Checkpoint, id: &str) -> bool {
    checkpoint.executed_tool_ids.iter().any(|done| done == id)
}

fn tool_label(name: &str) -> (String, String) {
    match tools::ToolName::parse(name) {
        Some(tool) => (tool.as_str().to_string(), tool.label().to_string()),
        None => (name.to_string(), name.to_string()),
    }
}

fn tool_message(call: &ToolCall, body: &Value) -> Message {
    Message::Tool {
        tool_call_id: call.id.clone(),
        name: call.name.clone(),
        content: serde_json::to_string(body)
            .unwrap_or_else(|_| "{\"error\":\"tool result\"}".to_string()),
    }
}

fn decode_checkpoint(turn: &Turn) -> Result<Option<Checkpoint>, Error> {
    match &turn.checkpoint {
        None | Some(Value::Null) => Ok(None),
        Some(value) => serde_json::from_value(value.clone())
            .map(Some)
            .map_err(|_| Error::Checkpoint),
    }
}

async fn save_checkpoint<S: HarnessStore>(
    store: &S,
    turn_id: Uuid,
    checkpoint: &Checkpoint,
) -> Result<(), Error> {
    let value = serde_json::to_value(checkpoint).map_err(|_| Error::Checkpoint)?;
    store
        .set_checkpoint(turn_id, value)
        .await
        .map_err(store_err)
}

async fn load_turn<S: HarnessStore>(store: &S, turn_id: Uuid) -> Result<Turn, Error> {
    let turns = store.running_turns().await.map_err(store_err)?;
    turns
        .into_iter()
        .find(|turn| turn.id == turn_id)
        .ok_or(Error::MissingTurn)
}

async fn write_audio(dir: &Path, turn_id: Uuid, bytes: &[u8]) -> Result<String, Error> {
    tokio::fs::create_dir_all(dir).await.map_err(|err| {
        tracing::warn!(error = %err, "audio dir");
        Error::Audio
    })?;
    let path = dir.join(turn_id.to_string());
    tokio::fs::write(&path, bytes).await.map_err(|err| {
        tracing::warn!(error = %err, "audio write");
        Error::Audio
    })?;
    path.to_str().map(str::to_string).ok_or(Error::Audio)
}

async fn emit<S: HarnessStore>(
    store: &S,
    user_id: Uuid,
    turn_id: Uuid,
    kind: Kind,
    payload: &impl Serialize,
) -> Result<(), Error> {
    let payload = serde_json::to_value(payload).map_err(|_| Error::Checkpoint)?;
    store
        .append_stream(user_id, turn_id, kind.as_str(), payload)
        .await
        .map_err(store_err)?;
    Ok(())
}

fn store_err(err: StoreError) -> Error {
    Error::Store(err.to_string())
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, VecDeque};
    use std::sync::Mutex;

    use async_trait::async_trait;
    use chrono::{TimeZone, Utc};
    use serde_json::json;
    use uuid::Uuid;

    use super::*;
    use crate::appdb::{
        AttendanceStatus, ChatRole, Durability, EventStatus, PlaceKind, TurnStatus,
    };
    use crate::config::Config;
    use crate::embed::{Embedder, Input};
    use crate::knowledge::Hit;
    use crate::llm::{Model, ModelStream, ToolCall};
    use crate::speech::Speech;

    fn event_id_args(id: Uuid) -> String {
        format!(r#"{{"event_id":"{id}"}}"#)
    }

    fn create_args(emoji: &str) -> String {
        format!(
            r#"{{"title":"Tennis","emoji":"{emoji}","starts_at":"2026-10-04T10:00:00Z","place_name":"Park","kind":"park","lat":52.2,"lon":21.0,"capacity":4,"description":"Hit","activity_tags":["tennis"]}}"#
        )
    }

    fn tool_call(id: &str, name: &str, arguments: &str) -> ToolCall {
        ToolCall {
            id: id.into(),
            name: name.into(),
            arguments: arguments.into(),
        }
    }

    struct ScriptedModel {
        streams: Mutex<VecDeque<Vec<StreamItem>>>,
        completes: Mutex<VecDeque<AssistantMessage>>,
        seen_stream: Mutex<Vec<Vec<Message>>>,
        seen_complete: Mutex<Vec<Vec<Message>>>,
    }

    impl ScriptedModel {
        fn new() -> Self {
            Self {
                streams: Mutex::new(VecDeque::new()),
                completes: Mutex::new(VecDeque::new()),
                seen_stream: Mutex::new(Vec::new()),
                seen_complete: Mutex::new(Vec::new()),
            }
        }

        fn push_stream(&self, items: Vec<StreamItem>) {
            self.streams.lock().expect("model").push_back(items);
        }

        fn push_tools(&self, calls: Vec<ToolCall>) {
            self.push_stream(vec![StreamItem::Done(AssistantMessage {
                content: None,
                tool_calls: calls,
            })]);
        }

        fn push_text(&self, text: &str) {
            self.push_stream(vec![
                StreamItem::TextDelta(text.to_string()),
                StreamItem::Done(AssistantMessage {
                    content: Some(text.to_string()),
                    tool_calls: Vec::new(),
                }),
            ]);
        }

        fn push_deltas(&self, parts: &[&str]) {
            let full: String = parts.concat();
            let mut items: Vec<_> = parts
                .iter()
                .map(|part| StreamItem::TextDelta((*part).to_string()))
                .collect();
            items.push(StreamItem::Done(AssistantMessage {
                content: Some(full),
                tool_calls: Vec::new(),
            }));
            self.push_stream(items);
        }

        fn push_empty(&self) {
            self.push_stream(vec![StreamItem::Done(AssistantMessage {
                content: None,
                tool_calls: Vec::new(),
            })]);
        }

        fn push_complete(&self, text: Option<&str>) {
            self.completes
                .lock()
                .expect("model")
                .push_back(AssistantMessage {
                    content: text.map(str::to_string),
                    tool_calls: Vec::new(),
                });
        }

        fn stream_messages(&self) -> Vec<Vec<Message>> {
            self.seen_stream.lock().expect("model").clone()
        }

        fn complete_messages(&self) -> Vec<Vec<Message>> {
            self.seen_complete.lock().expect("model").clone()
        }
    }

    #[async_trait]
    impl Model for ScriptedModel {
        async fn complete(&self, request: &CompletionRequest) -> anyhow::Result<AssistantMessage> {
            self.seen_complete
                .lock()
                .expect("model")
                .push(request.messages.clone());
            self.completes
                .lock()
                .expect("model")
                .pop_front()
                .ok_or_else(|| anyhow::anyhow!("no scripted completion"))
        }

        async fn stream(&self, request: &CompletionRequest) -> anyhow::Result<ModelStream> {
            self.seen_stream
                .lock()
                .expect("model")
                .push(request.messages.clone());
            let items = self
                .streams
                .lock()
                .expect("model")
                .pop_front()
                .ok_or_else(|| anyhow::anyhow!("no scripted stream"))?;
            Ok(Box::pin(futures::stream::iter(items.into_iter().map(Ok))))
        }
    }

    struct FakeSpeech {
        spoken: Mutex<Vec<String>>,
    }

    impl FakeSpeech {
        fn new() -> Self {
            Self {
                spoken: Mutex::new(Vec::new()),
            }
        }

        fn spoken(&self) -> Vec<String> {
            self.spoken.lock().expect("speech").clone()
        }
    }

    #[async_trait]
    impl Speech for FakeSpeech {
        async fn transcribe(
            &self,
            _audio: bytes::Bytes,
            _content_type: &str,
        ) -> Result<String, crate::speech::Error> {
            Err(crate::speech::Error::Transport("unused".into()))
        }

        async fn speak(&self, text: &str) -> Result<bytes::Bytes, crate::speech::Error> {
            self.spoken.lock().expect("speech").push(text.to_string());
            Ok(bytes::Bytes::from(text.to_string()))
        }
    }

    struct FakeEmbedder;

    #[async_trait]
    impl Embedder for FakeEmbedder {
        async fn embed(
            &self,
            _input: Input,
            texts: &[String],
        ) -> Result<Vec<Vec<f32>>, crate::embed::Error> {
            Ok(texts.iter().map(|_| vec![0.1, 0.2]).collect())
        }
    }

    struct FakeInner {
        user: User,
        profile: Profile,
        turn: Turn,
        memories: Vec<Memory>,
        messages: Vec<ChatMessage>,
        events: Vec<Event>,
        attendances: Vec<Attendance>,
        streams: Vec<crate::appdb::StreamEvent>,
        next_stream: i64,
        conversation: Conversation,
        remember_writes: u32,
        profile_writes: u32,
        create_writes: u32,
        join_calls: Vec<Uuid>,
        cancel_calls: Vec<Uuid>,
        complete_calls: Vec<Uuid>,
        forget_topics: Vec<String>,
        knowledge: Vec<(String, Vec<Hit>)>,
        next_event_id: Option<Uuid>,
        summaries: Vec<(String, Uuid)>,
    }

    struct FakeStore {
        inner: Mutex<FakeInner>,
    }

    impl FakeStore {
        fn new(user_id: Uuid, turn_id: Uuid, locale: &str, user_text: &str) -> Self {
            Self {
                inner: Mutex::new(FakeInner {
                    user: User {
                        id: user_id,
                        display_name: "Ada".into(),
                        locale: locale.into(),
                    },
                    profile: empty_profile(user_id),
                    turn: Turn {
                        id: turn_id,
                        user_id,
                        status: TurnStatus::Running,
                        user_text: Some(user_text.into()),
                        reply_text: None,
                        audio_path: None,
                        checkpoint: None,
                        error: None,
                    },
                    memories: Vec::new(),
                    messages: Vec::new(),
                    events: Vec::new(),
                    attendances: Vec::new(),
                    streams: Vec::new(),
                    next_stream: 1,
                    conversation: Conversation {
                        user_id,
                        summary: None,
                        summary_through: None,
                    },
                    remember_writes: 0,
                    profile_writes: 0,
                    create_writes: 0,
                    join_calls: Vec::new(),
                    cancel_calls: Vec::new(),
                    complete_calls: Vec::new(),
                    forget_topics: Vec::new(),
                    knowledge: Vec::new(),
                    next_event_id: None,
                    summaries: Vec::new(),
                }),
            }
        }

        fn lock(&self) -> std::sync::MutexGuard<'_, FakeInner> {
            self.inner.lock().expect("store")
        }

        fn remember_writes(&self) -> u32 {
            self.lock().remember_writes
        }

        fn profile_writes(&self) -> u32 {
            self.lock().profile_writes
        }

        fn create_writes(&self) -> u32 {
            self.lock().create_writes
        }

        fn join_calls(&self) -> Vec<Uuid> {
            self.lock().join_calls.clone()
        }

        fn cancel_calls(&self) -> Vec<Uuid> {
            self.lock().cancel_calls.clone()
        }

        fn complete_calls(&self) -> Vec<Uuid> {
            self.lock().complete_calls.clone()
        }

        fn forget_topics(&self) -> Vec<String> {
            self.lock().forget_topics.clone()
        }

        fn turn(&self) -> Turn {
            self.lock().turn.clone()
        }

        fn events(&self) -> Vec<Event> {
            self.lock().events.clone()
        }

        fn summaries(&self) -> Vec<(String, Uuid)> {
            self.lock().summaries.clone()
        }

        fn replay(&self, user_id: Uuid) -> Vec<crate::appdb::StreamEvent> {
            self.lock()
                .streams
                .iter()
                .filter(|event| event.user_id == user_id)
                .cloned()
                .collect()
        }

        fn set_mobility(&self, mobility: &str) {
            self.lock().profile.mobility = Some(mobility.to_string());
        }

        fn set_tags(&self, likes: &[&str], dislikes: &[&str]) {
            let mut inner = self.lock();
            inner.profile.likes = likes.iter().map(|tag| (*tag).to_string()).collect();
            inner.profile.dislikes = dislikes.iter().map(|tag| (*tag).to_string()).collect();
        }

        fn add_memory(&self, key: &str, value: &str) {
            self.lock().memories.push(Memory {
                id: Uuid::new_v4(),
                durability: Durability::LongTerm,
                key: key.into(),
                value: value.into(),
                quote: None,
                confidence: None,
                confirmed: true,
            });
        }

        fn set_hits(&self, query: &str, hits: Vec<Hit>) {
            self.lock().knowledge.push((query.to_string(), hits));
        }

        fn set_next_event_id(&self, id: Uuid) {
            self.lock().next_event_id = Some(id);
        }

        fn seed_event(&self, id: Uuid, capacity: Option<i32>, signed: i64) {
            let user_id = self.lock().user.id;
            self.lock()
                .events
                .push(blank_event(id, user_id, capacity, signed));
        }

        fn reopen(&self) {
            self.lock().turn.status = TurnStatus::Running;
        }
    }

    fn empty_profile(user_id: Uuid) -> Profile {
        Profile {
            user_id,
            age_band: None,
            gender: None,
            mobility: None,
            sportiness: None,
            bio: None,
            likes: Vec::new(),
            dislikes: Vec::new(),
            women_only: false,
            time_window: None,
            embedding: None,
            embedding_model: None,
        }
    }

    fn blank_event(id: Uuid, host_id: Uuid, capacity: Option<i32>, signed: i64) -> Event {
        Event {
            id,
            host_id,
            host_name: "Ada".into(),
            title: "Seeded".into(),
            emoji: "📍".into(),
            description: None,
            starts_at: Utc.with_ymd_and_hms(2026, 10, 4, 10, 0, 0).unwrap(),
            capacity,
            activity_tags: Vec::new(),
            promoted: false,
            status: EventStatus::Scheduled,
            women_only: false,
            place_name: "Park".into(),
            place_kind: PlaceKind::Park,
            latitude: 52.2,
            longitude: 21.0,
            signed_count: signed,
        }
    }

    fn topic_hit(topic: &str, text: &str) -> bool {
        text.to_lowercase().contains(topic)
    }

    fn retain_tags(tags: &mut Vec<String>, topic: &str, removed: &mut Vec<String>) {
        tags.retain(|tag| {
            if topic_hit(topic, tag) {
                removed.push(tag.clone());
                false
            } else {
                true
            }
        });
    }

    #[async_trait]
    impl HarnessStore for FakeStore {
        async fn running_turns(&self) -> Result<Vec<Turn>, StoreError> {
            let inner = self.lock();
            Ok(if inner.turn.status == TurnStatus::Running {
                vec![inner.turn.clone()]
            } else {
                Vec::new()
            })
        }

        async fn user(&self, id: Uuid) -> Result<User, StoreError> {
            let inner = self.lock();
            if inner.user.id == id {
                Ok(inner.user.clone())
            } else {
                Err(StoreError::new("user not found"))
            }
        }

        async fn profile(&self, user_id: Uuid) -> Result<Profile, StoreError> {
            let inner = self.lock();
            if inner.profile.user_id == user_id {
                Ok(inner.profile.clone())
            } else {
                Err(StoreError::new("profile not found"))
            }
        }

        async fn memories(&self, _user_id: Uuid) -> Result<Vec<Memory>, StoreError> {
            Ok(self.lock().memories.clone())
        }

        async fn conversation(&self, _user_id: Uuid) -> Result<Conversation, StoreError> {
            Ok(self.lock().conversation.clone())
        }

        async fn recent_turns(
            &self,
            user_id: Uuid,
            user_turns: i64,
        ) -> Result<Vec<ChatMessage>, StoreError> {
            let inner = self.lock();
            let mine: Vec<_> = inner
                .messages
                .iter()
                .filter(|message| message.user_id == user_id)
                .cloned()
                .collect();
            let n = usize::try_from(user_turns).unwrap_or(0);
            let user_idx: Vec<usize> = mine
                .iter()
                .enumerate()
                .filter(|(_, message)| message.role == ChatRole::User)
                .map(|(index, _)| index)
                .collect();
            if user_idx.is_empty() || n == 0 {
                return Ok(Vec::new());
            }
            let start = if user_idx.len() > n {
                user_idx[user_idx.len() - n]
            } else {
                0
            };
            Ok(mine[start..].to_vec())
        }

        async fn messages_before(
            &self,
            user_id: Uuid,
            before: Option<Uuid>,
            limit: i64,
        ) -> Result<Vec<ChatMessage>, StoreError> {
            let inner = self.lock();
            let mut mine: Vec<_> = inner
                .messages
                .iter()
                .filter(|message| message.user_id == user_id)
                .cloned()
                .collect();
            if let Some(id) = before {
                let Some(pos) = mine.iter().position(|message| message.id == id) else {
                    return Ok(Vec::new());
                };
                mine.truncate(pos);
            }
            let limit = usize::try_from(limit).unwrap_or(0);
            if mine.len() > limit {
                mine = mine.split_off(mine.len() - limit);
            }
            Ok(mine)
        }

        async fn count_user_messages(&self, user_id: Uuid) -> Result<i64, StoreError> {
            let inner = self.lock();
            let count = inner
                .messages
                .iter()
                .filter(|message| message.user_id == user_id && message.role == ChatRole::User)
                .count();
            Ok(i64::try_from(count).unwrap_or(i64::MAX))
        }

        async fn insert_message(
            &self,
            user_id: Uuid,
            role: ChatRole,
            body: &str,
        ) -> Result<ChatMessage, StoreError> {
            let mut inner = self.lock();
            let created_at = Utc.timestamp_opt(1_700_000_000, 0).unwrap()
                + chrono::Duration::milliseconds(inner.messages.len() as i64);
            let message = ChatMessage {
                id: Uuid::new_v4(),
                user_id,
                role,
                body: body.to_string(),
                created_at,
            };
            inner.messages.push(message.clone());
            Ok(message)
        }

        async fn set_checkpoint(&self, turn_id: Uuid, checkpoint: Value) -> Result<(), StoreError> {
            let mut inner = self.lock();
            if inner.turn.id != turn_id {
                return Err(StoreError::new("turn not found"));
            }
            inner.turn.checkpoint = Some(checkpoint);
            Ok(())
        }

        async fn finish_turn(
            &self,
            turn_id: Uuid,
            reply_text: &str,
            audio_path: Option<&str>,
        ) -> Result<(), StoreError> {
            let mut inner = self.lock();
            if inner.turn.id != turn_id {
                return Err(StoreError::new("turn not found"));
            }
            inner.turn.status = TurnStatus::Done;
            inner.turn.reply_text = Some(reply_text.to_string());
            inner.turn.audio_path = audio_path.map(str::to_string);
            Ok(())
        }

        async fn fail_turn(&self, turn_id: Uuid, message: &str) -> Result<(), StoreError> {
            let mut inner = self.lock();
            if inner.turn.id != turn_id {
                return Err(StoreError::new("turn not found"));
            }
            inner.turn.status = TurnStatus::Failed;
            inner.turn.error = Some(message.to_string());
            Ok(())
        }

        async fn append_stream(
            &self,
            user_id: Uuid,
            turn_id: Uuid,
            kind: &str,
            payload: Value,
        ) -> Result<i64, StoreError> {
            let mut inner = self.lock();
            let id = inner.next_stream;
            inner.next_stream += 1;
            inner.streams.push(crate::appdb::StreamEvent {
                id,
                user_id,
                turn_id: Some(turn_id),
                kind: kind.to_string(),
                payload,
                created_at: Utc::now(),
            });
            Ok(id)
        }

        async fn set_summary(
            &self,
            user_id: Uuid,
            summary: &str,
            through_message: Uuid,
        ) -> Result<(), StoreError> {
            let mut inner = self.lock();
            inner.conversation.user_id = user_id;
            inner.conversation.summary = Some(summary.to_string());
            inner.conversation.summary_through = Some(through_message);
            inner.summaries.push((summary.to_string(), through_message));
            Ok(())
        }

        async fn remember(&self, _user_id: Uuid, memory: NewMemory) -> Result<Memory, StoreError> {
            let mut inner = self.lock();
            inner.remember_writes += 1;
            let saved = Memory {
                id: Uuid::new_v4(),
                durability: memory.durability,
                key: memory.key,
                value: memory.value,
                quote: memory.quote,
                confidence: memory.confidence,
                confirmed: false,
            };
            inner.memories.push(saved.clone());
            Ok(saved)
        }

        async fn forget(&self, _user_id: Uuid, topic: &str) -> Result<ForgetResult, StoreError> {
            let mut inner = self.lock();
            inner.forget_topics.push(topic.to_string());
            let topic_l = topic.to_lowercase();
            let mut memories = Vec::new();
            inner.memories.retain(|memory| {
                let matched = topic_hit(&topic_l, &memory.key)
                    || topic_hit(&topic_l, &memory.value)
                    || memory
                        .quote
                        .as_ref()
                        .is_some_and(|quote| topic_hit(&topic_l, quote));
                if matched {
                    memories.push(memory.clone());
                }
                !matched
            });
            let mut tags = Vec::new();
            retain_tags(&mut inner.profile.likes, &topic_l, &mut tags);
            retain_tags(&mut inner.profile.dislikes, &topic_l, &mut tags);
            Ok(ForgetResult { memories, tags })
        }

        async fn apply_profile(
            &self,
            _user_id: Uuid,
            patch: &ProfilePatch,
        ) -> Result<Profile, StoreError> {
            let mut inner = self.lock();
            inner.profile_writes += 1;
            if let Some(name) = &patch.display_name {
                inner.user.display_name = name.clone();
            }
            if let Some(locale) = &patch.locale {
                inner.user.locale = locale.clone();
            }
            if let Some(value) = &patch.age_band {
                inner.profile.age_band = Some(value.clone());
            }
            if let Some(value) = &patch.gender {
                inner.profile.gender = Some(value.clone());
            }
            if let Some(value) = &patch.mobility {
                inner.profile.mobility = Some(value.clone());
            }
            if let Some(value) = patch.sportiness {
                inner.profile.sportiness = Some(value);
            }
            if let Some(value) = patch.women_only {
                inner.profile.women_only = value;
            }
            if let Some(window) = patch.time_window {
                inner.profile.time_window = Some(window);
            }
            if let Some(tag) = &patch.like_tag {
                if !inner.profile.likes.iter().any(|existing| existing == tag) {
                    inner.profile.likes.push(tag.clone());
                }
            }
            if let Some(tag) = &patch.dislike_tag {
                if !inner
                    .profile
                    .dislikes
                    .iter()
                    .any(|existing| existing == tag)
                {
                    inner.profile.dislikes.push(tag.clone());
                }
            }
            if let Some(bio) = &patch.bio {
                inner.profile.bio = Some(bio.clone());
            }
            Ok(inner.profile.clone())
        }

        async fn create_event(&self, host_id: Uuid, event: NewEvent) -> Result<Event, StoreError> {
            let mut inner = self.lock();
            inner.create_writes += 1;
            let id = inner.next_event_id.take().unwrap_or_else(Uuid::new_v4);
            let saved = Event {
                id,
                host_id,
                host_name: inner.user.display_name.clone(),
                title: event.title,
                emoji: event.emoji,
                description: event.description,
                starts_at: event.starts_at,
                capacity: event.capacity,
                activity_tags: event.activity_tags,
                promoted: false,
                status: EventStatus::Scheduled,
                women_only: event.women_only,
                place_name: event.place_name,
                place_kind: event.place_kind,
                latitude: event.latitude,
                longitude: event.longitude,
                signed_count: 0,
            };
            inner.events.push(saved.clone());
            Ok(saved)
        }

        async fn join_event(
            &self,
            event_id: Uuid,
            user_id: Uuid,
        ) -> Result<Attendance, StoreError> {
            let mut inner = self.lock();
            inner.join_calls.push(event_id);
            let Some(event) = inner.events.iter().find(|event| event.id == event_id) else {
                return Err(StoreError::new("event not found"));
            };
            if event
                .capacity
                .is_some_and(|cap| event.signed_count >= i64::from(cap))
            {
                return Err(StoreError::new("event is full"));
            }
            if let Some(pos) = inner.attendances.iter().position(|attendance| {
                attendance.event_id == event_id && attendance.user_id == user_id
            }) {
                if inner.attendances[pos].status != AttendanceStatus::Going {
                    inner.attendances[pos].status = AttendanceStatus::Going;
                    if let Some(event) = inner.events.iter_mut().find(|event| event.id == event_id)
                    {
                        event.signed_count += 1;
                    }
                }
                return Ok(inner.attendances[pos].clone());
            }
            if let Some(event) = inner.events.iter_mut().find(|event| event.id == event_id) {
                event.signed_count += 1;
            }
            let attendance = Attendance {
                event_id,
                user_id,
                status: AttendanceStatus::Going,
            };
            inner.attendances.push(attendance.clone());
            Ok(attendance)
        }

        async fn cancel_attendance(
            &self,
            event_id: Uuid,
            user_id: Uuid,
        ) -> Result<Attendance, StoreError> {
            let mut inner = self.lock();
            inner.cancel_calls.push(event_id);
            let Some(pos) = inner.attendances.iter().position(|attendance| {
                attendance.event_id == event_id && attendance.user_id == user_id
            }) else {
                return Err(StoreError::new("attendance not found"));
            };
            if inner.attendances[pos].status == AttendanceStatus::Going {
                if let Some(event) = inner.events.iter_mut().find(|event| event.id == event_id) {
                    event.signed_count = event.signed_count.saturating_sub(1);
                }
            }
            inner.attendances[pos].status = AttendanceStatus::Cancelled;
            Ok(inner.attendances[pos].clone())
        }

        async fn complete_attendance(
            &self,
            event_id: Uuid,
            user_id: Uuid,
        ) -> Result<Attendance, StoreError> {
            let mut inner = self.lock();
            inner.complete_calls.push(event_id);
            let Some(pos) = inner.attendances.iter().position(|attendance| {
                attendance.event_id == event_id && attendance.user_id == user_id
            }) else {
                return Err(StoreError::new("attendance not found"));
            };
            if inner.attendances[pos].status == AttendanceStatus::Going {
                if let Some(event) = inner.events.iter_mut().find(|event| event.id == event_id) {
                    event.signed_count = event.signed_count.saturating_sub(1);
                }
            }
            inner.attendances[pos].status = AttendanceStatus::Completed;
            Ok(inner.attendances[pos].clone())
        }

        async fn nearby_candidates(
            &self,
            _viewer: Uuid,
            _origin: rank::LatLng,
            _radius_m: f64,
            _bounds: Option<rank::BBox>,
            _now: DateTime<Utc>,
        ) -> Result<Vec<Candidate>, StoreError> {
            Ok(Vec::new())
        }

        async fn my_events(
            &self,
            _user_id: Uuid,
            _now: DateTime<Utc>,
        ) -> Result<Vec<Event>, StoreError> {
            Ok(self.lock().events.clone())
        }

        async fn people(
            &self,
            _viewer: Uuid,
            _origin: rank::LatLng,
        ) -> Result<Vec<Person>, StoreError> {
            Ok(Vec::new())
        }

        async fn search_messages(
            &self,
            user_id: Uuid,
            query: &str,
        ) -> Result<Vec<ChatMessage>, StoreError> {
            let query = query.to_lowercase();
            Ok(self
                .lock()
                .messages
                .iter()
                .filter(|message| {
                    message.user_id == user_id && message.body.to_lowercase().contains(&query)
                })
                .cloned()
                .collect())
        }

        async fn search_knowledge(
            &self,
            _embedder: &dyn Embedder,
            query: &Query,
        ) -> Result<Vec<Hit>, StoreError> {
            let inner = self.lock();
            Ok(inner
                .knowledge
                .iter()
                .find(|(text, _)| text == &query.text)
                .map(|(_, hits)| hits.clone())
                .unwrap_or_default())
        }
    }

    fn test_config(audio_dir: &Path) -> Config {
        let mut map = HashMap::<String, String>::new();
        for (key, value) in [
            ("LLM_BASE_URL", "http://127.0.0.1:9/v1"),
            ("LLM_API_KEY", "test-llm-key"),
            ("LLM_MODEL", "chat"),
            ("SPEECH_BASE_URL", "http://127.0.0.1:9/speech"),
            ("SPEECH_API_KEY", ""),
            ("STT_MODEL", "stt"),
            ("TTS_MODEL", "tts"),
            ("TTS_VOICE", "voice"),
            ("EMBED_BASE_URL", "http://127.0.0.1:9/embed"),
            ("EMBED_API_KEY", ""),
            ("EMBED_MODEL", "embed"),
            ("EMBED_QUERY_PREFIX", ""),
            ("EMBED_PASSAGE_PREFIX", ""),
            ("STYRTA_JWT_SECRET", "jwt-secret"),
            ("DATABASE_URL", "postgres://127.0.0.1/styrta"),
        ] {
            map.insert(key.to_string(), value.to_string());
        }
        map.insert(
            "STYRTA_AUDIO_DIR".to_string(),
            audio_dir.to_str().expect("utf8").to_string(),
        );
        Config::from_lookup(|key| map.get(key).cloned()).expect("config")
    }

    struct World {
        store: FakeStore,
        model: ScriptedModel,
        speech: FakeSpeech,
        embedder: FakeEmbedder,
        config: Config,
        user_id: Uuid,
        turn_id: Uuid,
    }

    impl World {
        fn new(locale: &str, user_text: &str) -> Self {
            let user_id = Uuid::new_v4();
            let turn_id = Uuid::new_v4();
            let audio_dir = std::env::temp_dir().join(format!("styrta-harness-{}", Uuid::new_v4()));
            std::fs::create_dir_all(&audio_dir).expect("audio dir");
            Self {
                store: FakeStore::new(user_id, turn_id, locale, user_text),
                model: ScriptedModel::new(),
                speech: FakeSpeech::new(),
                embedder: FakeEmbedder,
                config: test_config(&audio_dir),
                user_id,
                turn_id,
            }
        }

        fn services(&self) -> Services<'_, FakeStore> {
            Services {
                store: &self.store,
                model: &self.model,
                speech: &self.speech,
                embedder: &self.embedder,
                config: &self.config,
            }
        }

        async fn run(&self) -> Result<(), Error> {
            run_turn(self.services(), self.turn_id).await
        }
    }

    fn tool_bodies(messages: &[Message]) -> Vec<Value> {
        messages
            .iter()
            .filter_map(|message| match message {
                Message::Tool { content, .. } => serde_json::from_str(content).ok(),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn bad_tool_json_is_rejected_retried_and_does_not_write() {
        let world = World::new("pl", "zapamiętaj");
        world.model.push_tools(vec![
            tool_call("bad-json", "remember", "not-json"),
            tool_call("bad-name", "drop_table", "{}"),
        ]);
        world.model.push_text("Jasne.");
        world.run().await.expect("turn");
        assert_eq!(world.store.remember_writes(), 0);
        assert_eq!(world.store.profile_writes(), 0);
        assert_eq!(world.store.create_writes(), 0);
        assert!(world.store.join_calls().is_empty());
        let seen = world.model.stream_messages();
        assert!(seen.len() >= 2, "model was not retried");
        let bodies = tool_bodies(&seen[1]);
        assert!(
            bodies.iter().any(|body| body["error"]
                .as_str()
                .is_some_and(|text| text.contains("invalid"))),
            "{bodies:?}"
        );
        assert!(
            bodies.iter().any(|body| body["error"]
                .as_str()
                .is_some_and(|text| text.contains("unknown"))),
            "{bodies:?}"
        );
    }

    #[tokio::test]
    async fn remember_and_set_profile_each_hit_the_store_once() {
        let world = World::new("pl", "pamiętaj kawę");
        world.model.push_tools(vec![
            tool_call(
                "mem",
                "remember",
                r#"{"durability":"short_term","key":"drink","value":"coffee","quote":"lubię kawę"}"#,
            ),
            tool_call("prof", "set_profile", r#"{"locale":"en","like_tag":"chess"}"#),
        ]);
        world.model.push_text("Zapamiętane.");
        world.run().await.expect("turn");
        assert_eq!(world.store.remember_writes(), 1);
        assert_eq!(world.store.profile_writes(), 1);
        let memories = world.store.memories(world.user_id).await.expect("memories");
        assert_eq!(memories.len(), 1);
        assert_eq!(memories[0].value, "coffee");
        let profile = world.store.profile(world.user_id).await.expect("profile");
        assert_eq!(profile.likes, vec!["chess".to_string()]);
        assert_eq!(world.store.lock().user.locale, "en");
    }

    #[tokio::test]
    async fn forget_topic_removes_tennis_and_leaves_coffee() {
        let world = World::new("pl", "zapomnij tenis");
        world.store.add_memory("sport", "likes tennis");
        world.store.add_memory("drink", "likes coffee");
        world
            .store
            .set_tags(&["tennis", "coffee"], &["tennis", "rain"]);
        let tennis_id = world.store.memories(world.user_id).await.expect("mem")[0].id;
        world
            .model
            .push_tools(vec![tool_call("f", "forget", r#"{"topic":"tennis"}"#)]);
        world.model.push_text("Usunięte.");
        world.run().await.expect("turn");

        assert_eq!(world.store.forget_topics(), vec!["tennis".to_string()]);
        assert!(world
            .store
            .forget_topics()
            .iter()
            .all(|topic| topic != &tennis_id.to_string()));
        let memories = world.store.memories(world.user_id).await.expect("mem");
        assert_eq!(memories.len(), 1);
        assert_eq!(memories[0].value, "likes coffee");
        let profile = world.store.profile(world.user_id).await.expect("profile");
        assert_eq!(profile.likes, vec!["coffee".to_string()]);
        assert_eq!(profile.dislikes, vec!["rain".to_string()]);
    }

    #[tokio::test]
    async fn join_cancel_complete_and_create_event() {
        let world = World::new("pl", "załóż i dołącz");
        let event_id = Uuid::from_u128(42);
        world.store.set_next_event_id(event_id);
        let args = event_id_args(event_id);
        world.model.push_tools(vec![
            tool_call("c", "create_event", &create_args("🎾")),
            tool_call("j", "join_event", &args),
            tool_call("x", "cancel_attendance", &args),
            tool_call("d", "complete_attendance", &args),
        ]);
        world.model.push_text("Gotowe.");
        world.run().await.expect("turn");
        assert_eq!(world.store.create_writes(), 1);
        assert_eq!(world.store.events()[0].emoji, "🎾");
        assert_eq!(world.store.events()[0].id, event_id);
        assert_eq!(world.store.join_calls(), vec![event_id]);
        assert_eq!(world.store.cancel_calls(), vec![event_id]);
        assert_eq!(world.store.complete_calls(), vec![event_id]);
        let attendance = world.store.lock().attendances[0].clone();
        assert_eq!(attendance.status, AttendanceStatus::Completed);
    }

    #[tokio::test]
    async fn multi_grapheme_emoji_does_not_write() {
        let world = World::new("pl", "stwórz");
        world
            .model
            .push_tools(vec![tool_call("c", "create_event", &create_args("ab"))]);
        world.model.push_text("Emoji musi być jedno.");
        world.run().await.expect("turn");
        assert_eq!(world.store.create_writes(), 0);
        assert!(world.store.events().is_empty());
        let bodies = tool_bodies(&world.model.stream_messages()[1]);
        assert!(bodies[0]["error"]
            .as_str()
            .is_some_and(|text| text.contains("grapheme")));
    }

    #[tokio::test]
    async fn compaction_excludes_the_last_three_user_turns() {
        let world = World::new("en", "recent-user-3");
        let rows = [
            (ChatRole::User, "older-user-1"),
            (ChatRole::Assistant, "older-asst-1"),
            (ChatRole::User, "older-user-2"),
            (ChatRole::Assistant, "older-asst-2"),
            (ChatRole::User, "older-user-3"),
            (ChatRole::Assistant, "older-asst-3"),
            (ChatRole::User, "recent-user-1"),
            (ChatRole::Assistant, "recent-asst-1"),
            (ChatRole::User, "recent-user-2"),
            (ChatRole::Assistant, "recent-asst-2"),
        ];
        let mut through = None;
        for (role, body) in rows {
            let saved = world
                .store
                .insert_message(world.user_id, role, body)
                .await
                .expect("seed");
            if body == "older-asst-3" {
                through = Some(saved.id);
            }
        }
        world.model.push_text("recent-asst-3");
        world.model.push_complete(Some("Older summary."));
        world.run().await.expect("turn");

        let seen = world.model.complete_messages();
        assert_eq!(seen.len(), 1, "expected one summary call");
        let blob = serde_json::to_string(&seen[0]).expect("json");
        for kept in [
            "older-user-1",
            "older-user-2",
            "older-user-3",
            "older-asst-1",
        ] {
            assert!(blob.contains(kept), "{kept} missing from {blob}");
        }
        for dropped in [
            "recent-user-1",
            "recent-asst-1",
            "recent-user-2",
            "recent-asst-2",
            "recent-user-3",
            "recent-asst-3",
        ] {
            assert!(!blob.contains(dropped), "{dropped} leaked into {blob}");
        }
        assert_eq!(
            world.store.summaries(),
            vec![("Older summary.".to_string(), through.expect("through"))]
        );
    }

    #[tokio::test]
    async fn compaction_pages_past_the_newest_hundred_messages() {
        let world = World::new("en", "tail-user");
        for index in 0..62 {
            world
                .store
                .insert_message(world.user_id, ChatRole::User, &format!("page-user-{index}"))
                .await
                .expect("user");
            world
                .store
                .insert_message(
                    world.user_id,
                    ChatRole::Assistant,
                    &format!("page-asst-{index}"),
                )
                .await
                .expect("assistant");
        }
        world.model.push_text("tail-reply");
        world.model.push_complete(Some("paged"));
        world.run().await.expect("turn");
        let blob = serde_json::to_string(&world.model.complete_messages()[0]).expect("json");
        assert!(blob.contains("page-user-0"), "{blob}");
        assert!(blob.contains("page-user-59"), "{blob}");
        for dropped in ["page-user-60", "page-user-61", "tail-user", "tail-reply"] {
            assert!(!blob.contains(dropped), "{dropped} leaked into {blob}");
        }
    }

    #[tokio::test]
    async fn knowledge_hits_keep_only_returned_urls_and_empty_stays_empty() {
        let world = World::new("en", "what is in the library");
        world.store.set_hits(
            "parks",
            vec![Hit {
                title: "Parks".into(),
                page_url: "https://library.test/parks".into(),
                licence: "CC".into(),
                categories: vec!["park".into()],
                snippet: "A park.".into(),
                film_url: Some("https://library.test/parks.mp4".into()),
            }],
        );
        world.model.push_tools(vec![
            tool_call("k1", "search_knowledge", r#"{"query":"parks"}"#),
            tool_call("k2", "search_knowledge", r#"{"query":"nowhere"}"#),
        ]);
        world.model.push_text("The library has a parks page.");
        world.run().await.expect("turn");
        let bodies = tool_bodies(&world.model.stream_messages()[1]);
        assert_eq!(bodies.len(), 2);
        let urls = vec![
            bodies[0]["hits"][0]["page_url"].as_str().unwrap(),
            bodies[0]["hits"][0]["film_url"].as_str().unwrap(),
        ];
        assert_eq!(
            urls,
            [
                "https://library.test/parks",
                "https://library.test/parks.mp4"
            ]
        );
        assert_eq!(bodies[0]["hits"].as_array().unwrap().len(), 1);
        assert!(!bodies[0].to_string().contains("https://evil.example"));
        assert_eq!(bodies[1], json!({"hits": []}));
        assert!(!bodies[1].to_string().contains("http"));
    }

    #[tokio::test]
    async fn turn_emits_transcript_tool_reply_and_audio_in_order() {
        let world = World::new("pl", "cześć");
        world.store.set_mobility("wheelchair-private");
        world.model.push_tools(vec![tool_call(
            "mem",
            "remember",
            r#"{"durability":"long_term","key":"note","value":"hello","quote":"secret-quote"}"#,
        )]);
        world.model.push_deltas(&["Cze", "ść"]);
        world.run().await.expect("turn");

        let events = world.store.replay(world.user_id);
        let kinds: Vec<&str> = events.iter().map(|event| event.kind.as_str()).collect();
        let pos = |name: &str| kinds.iter().position(|kind| *kind == name).unwrap();
        assert!(pos("turn.started") < pos("transcript.ready"));
        assert!(pos("transcript.ready") < pos("tool.started"));
        assert!(pos("tool.started") < pos("tool.finished"));
        assert!(pos("tool.finished") < pos("reply.delta"));
        assert!(pos("reply.delta") < pos("reply.done"));
        assert!(pos("reply.done") < pos("audio.ready"));
        assert!(pos("audio.ready") < pos("turn.done"));
        assert_eq!(
            kinds.iter().filter(|kind| **kind == "reply.delta").count(),
            2
        );

        let blob: String = events
            .iter()
            .map(|event| format!("{} {}", event.kind, event.payload))
            .collect();
        assert!(!blob.contains("secret-quote"), "{blob}");
        assert!(!blob.contains("wheelchair-private"), "{blob}");

        let audio = events
            .iter()
            .find(|event| event.kind == "audio.ready")
            .expect("audio");
        let parsed: AudioReady = serde_json::from_value(audio.payload.clone()).expect("audio");
        assert_eq!(
            parsed.url,
            format!("/v1/chat/turns/{}/audio", world.turn_id)
        );
        let path = world.config.audio_dir.join(world.turn_id.to_string());
        assert_eq!(std::fs::read_to_string(path).expect("file"), "Cześć");
        assert_eq!(world.speech.spoken(), vec!["Cześć".to_string()]);

        let other = Uuid::new_v4();
        assert!(world.store.replay(other).is_empty());
        assert!(events.iter().all(|event| event.user_id == world.user_id));
    }

    #[tokio::test]
    async fn resume_from_checkpoint_does_not_join_again() {
        let world = World::new("pl", "dołącz");
        let event_id = Uuid::from_u128(7);
        world.store.seed_event(event_id, None, 0);
        let args = event_id_args(event_id);
        world
            .model
            .push_tools(vec![tool_call("join-1", "join_event", &args)]);
        world.model.push_text("Do zobaczenia");
        world.run().await.expect("first");
        assert_eq!(world.store.join_calls(), vec![event_id]);
        let checkpoint: Checkpoint =
            serde_json::from_value(world.store.turn().checkpoint.expect("checkpoint"))
                .expect("checkpoint");
        assert!(checkpoint.executed_tool_ids.iter().any(|id| id == "join-1"));

        world.store.reopen();
        world
            .model
            .push_tools(vec![tool_call("join-1", "join_event", &args)]);
        world.model.push_text("Do zobaczenia");
        resume_running(world.services()).await.expect("resume");
        assert_eq!(world.store.join_calls(), vec![event_id]);
    }

    #[tokio::test]
    async fn join_failure_is_a_tool_error() {
        let world = World::new("pl", "dołącz");
        let event_id = Uuid::from_u128(9);
        world.store.seed_event(event_id, Some(1), 1);
        world
            .model
            .push_tools(vec![tool_call("j", "join_event", &event_id_args(event_id))]);
        world.model.push_text("Miejsc brak.");
        world.run().await.expect("turn");
        assert_eq!(world.store.join_calls(), vec![event_id]);
        assert_eq!(world.store.turn().status, TurnStatus::Done);
        let bodies = tool_bodies(&world.model.stream_messages()[1]);
        assert!(bodies[0]["error"]
            .as_str()
            .is_some_and(|text| text.contains("full")));
    }

    #[tokio::test]
    async fn empty_reply_uses_repair_then_a_fixed_apology() {
        let world = World::new("pl", "hej");
        world.model.push_empty();
        world.model.push_complete(None);
        world.run().await.expect("turn");
        assert_eq!(world.model.complete_messages().len(), 1);
        assert_eq!(world.speech.spoken(), vec![apology("pl").to_string()]);
        let spoken = world.speech.spoken()[0].clone();
        assert!(!spoken.contains("http"));
        assert!(!spoken.to_lowercase().contains("http"));
    }
}
