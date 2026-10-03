use crate::appdb::{ChatMessage, ChatRole, Durability, Memory, Profile};
use crate::harness::RECENT_USER_TURNS;
use crate::llm::Message;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    NotImplemented,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("not implemented")
    }
}

impl std::error::Error for Error {}

pub(crate) fn english_locale(locale: &str) -> bool {
    locale.trim().eq_ignore_ascii_case("en")
}

pub fn messages(
    locale: &str,
    profile: &Profile,
    memories: &[Memory],
    summary: Option<&str>,
    recent: &[ChatMessage],
    user_text: &str,
) -> Result<Vec<Message>, Error> {
    let mut out = vec![
        Message::System {
            content: system_prompt(locale).to_string(),
        },
        Message::System {
            content: context_block(profile, memories),
        },
    ];
    if let Some(summary) = summary.map(str::trim).filter(|text| !text.is_empty()) {
        out.push(Message::System {
            content: format!("Conversation summary:\n{summary}"),
        });
    }
    for message in recent_window(recent) {
        out.push(match message.role {
            ChatRole::User => Message::User {
                content: message.body.clone(),
            },
            ChatRole::Assistant => Message::Assistant {
                content: Some(message.body.clone()),
                tool_calls: Vec::new(),
            },
        });
    }
    out.push(Message::User {
        content: user_text.to_string(),
    });
    Ok(out)
}

fn system_prompt(locale: &str) -> &'static str {
    if english_locale(locale) {
        "You are Styrta, a helper for nearby meetups. Reply in English, briefly.\n\
Call create_event, join_event, cancel_attendance, and complete_attendance only after the user asked.\n\
If search_knowledge returns nothing, say the library has nothing. Do not invent URLs or events."
    } else {
        "Jesteś Styrtą i pomagasz umawiać się na spotkania w pobliżu. Odpowiadaj po polsku, krótko.\n\
create_event, join_event, cancel_attendance i complete_attendance wolno wywołać tylko wtedy, gdy użytkownik o to poprosił.\n\
Gdy search_knowledge nic nie zwróci, powiedz, że w bibliotece nic nie ma. Nie wymyślaj adresów URL ani wydarzeń."
    }
}

fn context_block(profile: &Profile, memories: &[Memory]) -> String {
    let mut text = String::from("Profile:\n");
    push_opt(&mut text, "age_band", &profile.age_band);
    push_opt(&mut text, "gender", &profile.gender);
    push_opt(&mut text, "mobility", &profile.mobility);
    if let Some(sportiness) = profile.sportiness {
        text.push_str(&format!("sportiness: {sportiness}\n"));
    }
    if !profile.likes.is_empty() {
        text.push_str(&format!("likes: {}\n", profile.likes.join(", ")));
    }
    if !profile.dislikes.is_empty() {
        text.push_str(&format!("dislikes: {}\n", profile.dislikes.join(", ")));
    }
    text.push_str(&format!("women_only: {}\n", profile.women_only));
    if let Some(window) = profile.time_window {
        text.push_str(&format!(
            "time_window: {}-{}\n",
            window.start_minute, window.end_minute
        ));
    }
    push_opt(&mut text, "bio", &profile.bio);
    text.push_str("Memories:\n");
    if memories.is_empty() {
        text.push_str("none\n");
        return text;
    }
    for memory in memories {
        text.push_str(&format!(
            "- {} | {} | {}",
            durability_label(memory.durability),
            memory.key,
            memory.value
        ));
        if let Some(quote) = &memory.quote {
            text.push_str(" | ");
            text.push_str(quote);
        }
        if let Some(confidence) = memory.confidence {
            text.push_str(&format!(" | {confidence}"));
        }
        text.push('\n');
    }
    text
}

fn push_opt(text: &mut String, label: &str, value: &Option<String>) {
    if let Some(value) = value {
        text.push_str(label);
        text.push_str(": ");
        text.push_str(value);
        text.push('\n');
    }
}

fn durability_label(durability: Durability) -> &'static str {
    match durability {
        Durability::LongTerm => "long_term",
        Durability::ShortTerm => "short_term",
    }
}

fn recent_window(recent: &[ChatMessage]) -> &[ChatMessage] {
    let mut remaining = RECENT_USER_TURNS;
    for (index, message) in recent.iter().enumerate().rev() {
        if message.role == ChatRole::User {
            remaining -= 1;
            if remaining == 0 {
                return &recent[index..];
            }
        }
    }
    recent
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use uuid::Uuid;

    use super::*;

    fn profile() -> Profile {
        Profile {
            user_id: Uuid::nil(),
            age_band: Some("30s".into()),
            gender: None,
            mobility: Some("walks".into()),
            sportiness: Some(1),
            bio: None,
            likes: vec!["coffee".into()],
            dislikes: Vec::new(),
            women_only: false,
            time_window: None,
            embedding: None,
            embedding_model: None,
        }
    }

    fn chat(role: ChatRole, body: &str) -> ChatMessage {
        ChatMessage {
            id: Uuid::new_v4(),
            user_id: Uuid::nil(),
            role,
            body: body.into(),
            created_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
        }
    }

    fn system(messages: &[Message], index: usize) -> &str {
        match &messages[index] {
            Message::System { content } => content,
            other => panic!("expected system at {index}, got {other:?}"),
        }
    }

    #[test]
    fn polish_when_locale_is_blank_or_pl() {
        for locale in ["", "pl", "PL", "  "] {
            let messages = messages(locale, &profile(), &[], None, &[], "cześć").unwrap();
            let prompt = system(&messages, 0);
            assert!(
                prompt.contains("w bibliotece nic nie ma"),
                "{locale}: {prompt}"
            );
            assert!(prompt.contains("create_event"), "{prompt}");
            assert!(prompt.len() < 800, "{}", prompt.len());
            assert!(!prompt.contains("library has nothing"), "{prompt}");
        }
    }

    #[test]
    fn english_when_locale_is_en() {
        let messages = messages("en", &profile(), &[], None, &[], "hi").unwrap();
        let prompt = system(&messages, 0);
        assert!(prompt.contains("library has nothing"), "{prompt}");
        assert!(prompt.contains("only after the user asked"), "{prompt}");
        assert!(!prompt.contains("bibliotece"), "{prompt}");
    }

    #[test]
    fn order_is_prompt_then_profile_summary_recent_and_new_user_text() {
        let memory = Memory {
            id: Uuid::nil(),
            durability: Durability::LongTerm,
            key: "drink".into(),
            value: "tennis-memory".into(),
            quote: Some("quoted-line".into()),
            confidence: None,
            confirmed: true,
        };
        let recent = vec![
            chat(ChatRole::User, "user-1"),
            chat(ChatRole::Assistant, "reply-1"),
            chat(ChatRole::User, "user-2"),
            chat(ChatRole::Assistant, "reply-2"),
            chat(ChatRole::User, "user-3"),
            chat(ChatRole::Assistant, "reply-3"),
            chat(ChatRole::User, "user-4"),
            chat(ChatRole::Assistant, "reply-4"),
        ];
        let messages = messages(
            "pl",
            &profile(),
            &[memory],
            Some("sum-text"),
            &recent,
            "new-text",
        )
        .unwrap();

        assert!(system(&messages, 0).contains("bibliotece"));
        let context = system(&messages, 1);
        assert!(context.contains("age_band: 30s"), "{context}");
        assert!(context.contains("tennis-memory"), "{context}");
        assert!(context.contains("quoted-line"), "{context}");
        assert!(system(&messages, 2).contains("sum-text"));

        let bodies: Vec<&str> = messages
            .iter()
            .skip(3)
            .map(|message| match message {
                Message::User { content } => content.as_str(),
                Message::Assistant { content, .. } => content.as_deref().unwrap(),
                Message::System { .. } | Message::Tool { .. } => panic!("chat row"),
            })
            .collect();
        assert_eq!(
            bodies,
            vec!["user-2", "reply-2", "user-3", "reply-3", "user-4", "reply-4", "new-text",]
        );
    }
}
