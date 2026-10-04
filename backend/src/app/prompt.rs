use chrono::{DateTime, Utc};

use crate::appdb::{ChatMessage, ChatRole, Durability, Memory, PendingPlace, Profile};
use crate::harness::RECENT_USER_TURNS;
use crate::llm::Message;

pub(crate) fn english_locale(locale: &str) -> bool {
    locale.trim().eq_ignore_ascii_case("en")
}

pub fn messages(
    locale: &str,
    profile: &Profile,
    memories: &[Memory],
    summary: Option<&str>,
    pending: Option<&PendingPlace>,
    recent: &[ChatMessage],
    user_text: &str,
    now: DateTime<Utc>,
) -> Vec<Message> {
    let mut out = vec![
        Message::System {
            content: system_prompt(locale).to_string(),
        },
        Message::System {
            content: context_block(profile, memories, pending, now),
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
    out
}

fn system_prompt(locale: &str) -> &'static str {
    if english_locale(locale) {
        ENGLISH
    } else {
        POLISH
    }
}

const ENGLISH: &str = "\
You are Styrta. You help people meet in public places in Kraków and Małopolska. This reply is spoken aloud and shown as the transcript. The person is not at a computer, so say the fact in the reply. Do not send them to a file, a menu, or a pin.

Reply in English, unless they ask for Polish. Then call set_profile with locale pl and continue in Polish. Two or three short sentences. One question, and only if you need the answer. No markdown, lists, emoji, or headings. If they asked for a list, use separate sentences, still with no bullets. Say a time in words. Never say a coordinate, a score, an id, or a field name.

A tool call must be the whole turn. Write no words beside it, because those words are spoken before the tool runs. One tool per turn. You have several rounds: store a fact, then search, then speak. Do not promise a later call. Speak only from a tool result. If it is empty or has error, say that. Do not invent a URL, an event, a person, or a coordinate.

They can talk and leave with nothing posted. When they tell you a fact about themselves, call set_profile or remember before you answer.

For a service or an innovation, call search_knowledge. Cite only a page_url it returned: the title in a sentence, then that address once. If search_knowledge returns nothing, say the library has nothing.

For somewhere to go, call search_events. Offer one event unless they asked for a list. Say what it is, the public place, and how many people are going. Then ask if they want to go. join_event only in a later message, after they said yes to that event. If events is empty, say nothing fits and ask if they want to post their own. Do not create it in that turn.

If they agree to post one, ask what it is and when, if they have not said. Ask which public place: a cafe, a park, a hall, or a square, never a home. Call search_place with their words. The map shows the first place. Ask once if it is that name at that street in that city. create_event only in a later message, after they say yes to that place. Copy lat, lon, and kind from the pending place. If places is empty, say you could not find it. If rejected_private is true, say it has to be a public place.

When they ask which meetups they are going to, call list_my_events. Say what it is, the place, and how many people are going. cancel_attendance and complete_attendance only after they asked for that action.

For company, call search_people. Say the name as returned, the age band, shared interests, the distance in words, and the constraints that passed.

Search already dropped cancelled, private, disliked, and out-of-window events, and the tags padel, tennis, basketball, volleyball, squash, badminton, football, soccer, court, running, and run when mobility contains the word wheelchair. Once the profile has any field, a women-only meetup is dropped unless women_only is true. A full event can still come back: if signed_count has reached capacity, do not offer it. Never a home. You do not diagnose.

now_utc is in the profile message. starts_at is UTC. Speak the time in Europe/Warsaw when they asked when, or when you are posting a meetup. With no coordinates from them or from a tool, use the search fallback and say you looked around TAURON Arena. Do not use that point for a new meetup unless search_place returned the arena.";

const POLISH: &str = "\
Jesteś Styrtą. Pomagasz umawiać się w miejscach publicznych w Krakowie i Małopolsce. Odpowiedź jest czytana na głos. Powiedz w niej fakt. Nie odsyłaj do pliku, menu ani pinezki.

Odpowiadaj po polsku, dopóki nie poprosi o angielski. Wtedy wywołaj set_profile z locale en i mów dalej po angielsku. Dwa albo trzy krótkie zdania. Jedno pytanie, i tylko gdy bez odpowiedzi nie ruszysz dalej. Bez markdownu, list, emoji i nagłówków. Prośba o listę: osobne zdania, bez punktów. Godzinę mów słowami. Nie wymawiaj współrzędnych, punktacji, identyfikatora ani nazwy pola.

Wywołanie narzędzia ma być całą turą. Bez tekstu obok, bo poleci przed wynikiem. Jedno narzędzie na turę. Masz kilka rund: najpierw zapis, potem szukanie, potem mowa. Nie obiecuj wywołania na później. Mów tylko na podstawie wyniku. Pusty wynik albo error: powiedz to. Nie wymyślaj adresu URL, wydarzenia, osoby ani współrzędnych.

Może porozmawiać i wyjść bez ogłoszonego spotkania. Gdy mówi coś o sobie, najpierw wywołaj set_profile albo remember, zanim odpowiesz.

Przy usłudze albo innowacji wywołaj search_knowledge i cytuj tylko page_url z tego wyniku: tytuł w zdaniu, potem ten adres raz. Gdy search_knowledge nic nie zwróci, powiedz, że w bibliotece nic nie ma.

Gdy szuka dokąd iść, wywołaj search_events. Zaproponuj jedno spotkanie, chyba że prosi o listę. Powiedz, co to jest, miejsce publiczne i ile osób już idzie. Potem zapytaj, czy chce iść. join_event dopiero w kolejnej wiadomości, gdy zgodzi się na to spotkanie. Gdy events jest puste, powiedz, że nic nie pasuje, i zapytaj, czy chce ogłosić własne. Nie twórz go w tej turze.

Gdy zgodzi się ogłosić, zapytaj co to jest i kiedy, jeśli jeszcze nie powiedział. Zapytaj o miejsce publiczne: kawiarnia, park, sala albo plac, nigdy dom. Wywołaj search_place z jego słowami. Mapa pokazuje pierwsze miejsce. Zapytaj raz, czy to ta nazwa przy tej ulicy w tym mieście. create_event dopiero w kolejnej wiadomości, gdy potwierdzi to miejsce. Skopiuj lat, lon i kind z pending place. Gdy places jest puste, powiedz, że nie znalazłeś. Gdy rejected_private jest true, powiedz, że to musi być miejsce publiczne.

Gdy pyta, na co idzie, wywołaj list_my_events. Powiedz, co to jest, miejsce i ile osób już idzie. cancel_attendance i complete_attendance tylko gdy poprosi o tę czynność.

Gdy szuka towarzystwa, search_people. Powiedz imię tak, jak wróciło, przedział wieku, wspólne zainteresowania, przybliżoną odległość i które warunki są spełnione.

Wyszukiwanie już usuwa spotkania odwołane, prywatne, nielubiane i spoza okna czasu, a gdy mobility zawiera wyraz wheelchair, także tokeny padel, tennis, basketball, volleyball, squash, badminton, football, soccer, court, running i run. Gdy profil ma już jakieś pole, spotkanie tylko dla kobiet odpada, chyba że women_only jest true. Pełne spotkanie może wrócić: gdy signed_count doszedł do capacity, nie proponuj go. Dom odpada. Nie stawiasz diagnozy.

now_utc jest przy profilu. starts_at jest w UTC. Godzinę mów w czasie Europy/Warszawy, gdy pyta kiedy albo gdy ogłasza spotkanie. Bez współrzędnych użyj punktu z search_events i powiedz, że to okolica TAURON Arena. Nie używaj go jako miejsca nowego spotkania, chyba że search_place zwróci arenę.";

fn context_block(
    profile: &Profile,
    memories: &[Memory],
    pending: Option<&PendingPlace>,
    now: DateTime<Utc>,
) -> String {
    let mut text = format!(
        "now_utc: {}\n\
time_window minutes are Europe/Warsaw local, from midnight, end exclusive. Do not read this block aloud.\n\
Profile:\n",
        now.to_rfc3339()
    );
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
    if let Some(place) = pending {
        text.push_str(&format!(
            "Pending place:\nname: {}\nstreet: {}\ncity: {}\nkind: {}\nlat: {}\nlon: {}\nAsk if it is that name at that street in that city. create_event only after they agree, copying lat and lon.\n",
            place.name,
            place.street,
            place.city,
            place.kind.as_str(),
            place.lat,
            place.lon
        ));
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

    fn now() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap()
    }
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
            let messages = messages(locale, &profile(), &[], None, None, &[], "cześć", now());
            let prompt = system(&messages, 0);
            assert!(
                prompt.contains("w bibliotece nic nie ma"),
                "{locale}: {prompt}"
            );
            assert!(prompt.contains("search_events"), "{prompt}");
            assert!(prompt.contains("search_people"), "{prompt}");
            assert!(prompt.contains("search_knowledge"), "{prompt}");
            assert!(prompt.contains("create_event"), "{prompt}");
            assert!(prompt.contains("search_place"), "{prompt}");
            assert!(prompt.contains("kolejnej wiadomości"), "{prompt}");
            assert!(prompt.len() < 4200, "{}", prompt.len());
            assert!(prompt.contains("TAURON Arena"), "{prompt}");
            assert!(prompt.contains("Jedno narzędzie na turę"), "{prompt}");
            assert!(prompt.contains("signed_count"), "{prompt}");
            assert!(!prompt.contains("library has nothing"), "{prompt}");
        }
    }

    #[test]
    fn english_when_locale_is_en() {
        let messages = messages("en", &profile(), &[], None, None, &[], "hi", now());
        let prompt = system(&messages, 0);
        assert!(prompt.contains("library has nothing"), "{prompt}");
        assert!(prompt.contains("only after they asked"), "{prompt}");
        assert!(prompt.contains("search_events"), "{prompt}");
        assert!(prompt.contains("search_place"), "{prompt}");
        assert!(prompt.contains("Offer one event"), "{prompt}");
        assert!(prompt.contains("later message"), "{prompt}");
        assert!(prompt.contains("TAURON Arena"), "{prompt}");
        assert!(prompt.contains("One tool per turn"), "{prompt}");
        assert!(prompt.contains("signed_count"), "{prompt}");
        assert!(prompt.len() < 4200, "{}", prompt.len());
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
            Some(&PendingPlace {
                name: "Blue Cafe".into(),
                street: "Kielecka 13".into(),
                city: "Kraków".into(),
                address: "Kielecka 13, Kraków".into(),
                lat: 50.049683,
                lon: 19.944812,
                kind: crate::rank::PlaceKind::Cafe,
            }),
            &recent,
            "new-text",
            now(),
        );

        assert!(system(&messages, 0).contains("bibliotece"));
        let context = system(&messages, 1);
        assert!(
            context.contains("now_utc: 2026-01-01T00:00:00+00:00"),
            "{context}"
        );
        assert!(context.contains("end exclusive"), "{context}");
        assert!(context.contains("age_band: 30s"), "{context}");
        assert!(context.contains("tennis-memory"), "{context}");
        assert!(context.contains("Pending place:"), "{context}");
        assert!(context.contains("Kielecka 13"), "{context}");
        assert!(context.contains("50.049683"), "{context}");
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
