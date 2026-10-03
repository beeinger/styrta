use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

pub const HEARTBEAT: Duration = Duration::from_secs(15);
pub const HEARTBEAT_COMMENT: &str = ": ping\n\n";
pub const POLL_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Encode,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("could not encode event")
    }
}

impl std::error::Error for Error {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    TurnStarted,
    TranscriptReady,
    ToolStarted,
    ToolFinished,
    ReplyDelta,
    ReplyDone,
    AudioReady,
    TurnDone,
    TurnFailed,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TurnStarted => "turn.started",
            Self::TranscriptReady => "transcript.ready",
            Self::ToolStarted => "tool.started",
            Self::ToolFinished => "tool.finished",
            Self::ReplyDelta => "reply.delta",
            Self::ReplyDone => "reply.done",
            Self::AudioReady => "audio.ready",
            Self::TurnDone => "turn.done",
            Self::TurnFailed => "turn.failed",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "turn.started" => Self::TurnStarted,
            "transcript.ready" => Self::TranscriptReady,
            "tool.started" => Self::ToolStarted,
            "tool.finished" => Self::ToolFinished,
            "reply.delta" => Self::ReplyDelta,
            "reply.done" => Self::ReplyDone,
            "audio.ready" => Self::AudioReady,
            "turn.done" => Self::TurnDone,
            "turn.failed" => Self::TurnFailed,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub id: i64,
    pub kind: Kind,
    pub data: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TaggedFrame {
    pub user_id: Uuid,
    pub frame: Frame,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resume {
    /// Replay events with `id` greater than this value, then follow.
    After(i64),
    /// No `Last-Event-ID`: follow events appended after the cursor is primed.
    Live,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnRef {
    pub turn_id: Uuid,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptReady {
    pub turn_id: Uuid,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolStarted {
    pub turn_id: Uuid,
    pub tool: String,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolFinished {
    pub turn_id: Uuid,
    pub tool: String,
    pub ok: bool,
    pub event_id: Option<Uuid>,
    pub hit_count: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplyDelta {
    pub turn_id: Uuid,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplyDone {
    pub turn_id: Uuid,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioReady {
    pub turn_id: Uuid,
    pub url: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnFailed {
    pub turn_id: Uuid,
    pub error: String,
}

pub fn heartbeat_due(elapsed: Duration) -> bool {
    elapsed >= HEARTBEAT
}

/// `None` when the header is present but not a non-negative integer.
pub fn resume_from(header: Option<&str>) -> Option<Resume> {
    let Some(value) = header else {
        return Some(Resume::Live);
    };
    let value = value.trim();
    if value.is_empty() {
        return Some(Resume::Live);
    }
    match value.parse::<i64>() {
        Ok(id) if id >= 0 => Some(Resume::After(id)),
        _ => None,
    }
}

pub fn frames_for_user(user_id: Uuid, frames: &[TaggedFrame]) -> Vec<&Frame> {
    frames
        .iter()
        .filter(|frame| frame.user_id == user_id)
        .map(|frame| &frame.frame)
        .collect()
}

/// SSE body for one user, strictly after `last_event_id`.
pub fn render(user_id: Uuid, last_event_id: i64, frames: &[TaggedFrame]) -> Result<String, Error> {
    let mut selected: Vec<&Frame> = frames_for_user(user_id, frames)
        .into_iter()
        .filter(|frame| frame.id > last_event_id)
        .collect();
    selected.sort_by_key(|frame| frame.id);
    let mut out = String::new();
    for frame in selected {
        out.push_str(&encode(frame)?);
    }
    Ok(out)
}

pub fn encode(frame: &Frame) -> Result<String, Error> {
    let data = serde_json::to_string(&frame.data).map_err(|_| Error::Encode)?;
    let mut out = format!("id: {}\nevent: {}\n", frame.id, frame.kind.as_str());
    for line in data.split('\n') {
        out.push_str("data: ");
        out.push_str(line);
        out.push('\n');
    }
    out.push('\n');
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tagged(user_id: Uuid, id: i64, kind: Kind, data: Value) -> TaggedFrame {
        TaggedFrame {
            user_id,
            frame: Frame { id, kind, data },
        }
    }

    #[test]
    fn encode_snapshot() {
        let frame = Frame {
            id: 42,
            kind: Kind::ToolFinished,
            data: json!({
                "turn_id": "11111111-1111-1111-1111-111111111111",
                "tool": "create_event",
                "ok": true,
                "event_id": "22222222-2222-2222-2222-222222222222",
                "hit_count": null,
            }),
        };
        assert_eq!(
            encode(&frame).unwrap(),
            concat!(
                "id: 42\n",
                "event: tool.finished\n",
                "data: {\"event_id\":\"22222222-2222-2222-2222-222222222222\",\"hit_count\":null,\"ok\":true,\"tool\":\"create_event\",\"turn_id\":\"11111111-1111-1111-1111-111111111111\"}\n",
                "\n",
            )
        );
    }

    #[test]
    fn heartbeat_comment_and_interval() {
        assert_eq!(HEARTBEAT, Duration::from_secs(15));
        assert_eq!(POLL_INTERVAL, Duration::from_millis(200));
        assert_eq!(HEARTBEAT_COMMENT, ": ping\n\n");
        assert!(!heartbeat_due(Duration::from_secs(14)));
        assert!(heartbeat_due(HEARTBEAT));
        assert!(heartbeat_due(Duration::from_secs(16)));
    }

    #[test]
    fn resume_from_last_event_id() {
        assert_eq!(resume_from(None), Some(Resume::Live));
        assert_eq!(resume_from(Some("")), Some(Resume::Live));
        assert_eq!(resume_from(Some("  ")), Some(Resume::Live));
        assert_eq!(resume_from(Some("7")), Some(Resume::After(7)));
        assert_eq!(resume_from(Some(" 4 ")), Some(Resume::After(4)));
        assert_eq!(resume_from(Some("-1")), None);
        assert_eq!(resume_from(Some("nope")), None);
    }

    #[test]
    fn stream_for_user_a_does_not_emit_user_b() {
        let user_a = Uuid::from_u128(1);
        let user_b = Uuid::from_u128(2);
        let frames = vec![
            tagged(user_a, 1, Kind::TurnStarted, json!({"who": "alpha"})),
            tagged(
                user_b,
                2,
                Kind::TranscriptReady,
                json!({"text": "bravo-secret"}),
            ),
            tagged(user_a, 4, Kind::TurnDone, json!({"who": "alpha-done"})),
            tagged(user_a, 3, Kind::ReplyDone, json!({"text": "alpha-reply"})),
        ];

        let mine = frames_for_user(user_a, &frames);
        assert_eq!(mine.len(), 3);
        assert!(mine.iter().all(|frame| frame.id != 2));

        let rendered = render(user_a, 0, &frames).unwrap();
        assert!(!rendered.contains("bravo-secret"));
        assert!(rendered.contains("event: turn.started\n"));
        assert!(rendered.contains("alpha-reply"));
        assert!(rendered.contains("alpha-done"));
        let started = rendered.find("event: turn.started").unwrap();
        let reply = rendered.find("alpha-reply").unwrap();
        let done = rendered.find("alpha-done").unwrap();
        assert!(started < reply && reply < done);

        let replay = render(user_a, 1, &frames).unwrap();
        assert!(!replay.contains("event: turn.started"));
        assert!(!replay.contains("bravo-secret"));
        assert!(replay.contains("alpha-reply"));
        assert!(replay.contains("id: 3\n"));
        assert!(replay.contains("id: 4\n"));
    }
}
