use std::collections::HashMap;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::rank;
pub use crate::rank::{EventStatus, PlaceKind, TimeWindow};

/// Mean Earth radius in meters. Distance uses haversine on float8 lat/lon.
const EARTH_RADIUS_M: f64 = 6_371_000.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    NotImplemented,
    NotFound,
    EmptyTopic,
    Capacity,
    Invalid,
    Database(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotImplemented => f.write_str("not implemented"),
            Self::NotFound => f.write_str("not found"),
            Self::EmptyTopic => f.write_str("empty topic"),
            Self::Capacity => f.write_str("event is full"),
            Self::Invalid => f.write_str("invalid value"),
            Self::Database(message) => write!(f, "database error: {message}"),
        }
    }
}

impl std::error::Error for Error {}

#[derive(Clone)]
pub struct Store {
    pool: PgPool,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Store")
    }
}

impl Store {
    pub async fn connect(database_url: &str) -> Result<Self, Error> {
        let pool = PgPool::connect(database_url).await.map_err(db)?;
        migrate(&pool).await?;
        Ok(Self { pool })
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub async fn create_user(&self, display_name: &str, locale: &str) -> Result<User, Error> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        let (id, display_name, locale): (Uuid, String, String) = sqlx::query_as(
            r#"
            INSERT INTO users (display_name, locale)
            VALUES ($1, $2)
            RETURNING id, display_name, locale
            "#,
        )
        .bind(display_name)
        .bind(locale)
        .fetch_one(&mut *tx)
        .await
        .map_err(db)?;
        sqlx::query("INSERT INTO profiles (user_id) VALUES ($1)")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        sqlx::query("INSERT INTO conversations (user_id) VALUES ($1)")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        tx.commit().await.map_err(db)?;
        Ok(User {
            id,
            display_name,
            locale,
        })
    }

    pub async fn user(&self, id: Uuid) -> Result<User, Error> {
        sqlx::query_as("SELECT id, display_name, locale FROM users WHERE id = $1")
            .bind(id)
            .fetch_one(&self.pool)
            .await
            .map_err(db)
    }

    pub async fn profile(&self, user_id: Uuid) -> Result<Profile, Error> {
        let row: ProfileRow = sqlx::query_as(&profile_sql())
            .bind(user_id)
            .fetch_one(&self.pool)
            .await
            .map_err(db)?;
        profile_from_row(row)
    }

    pub async fn apply_profile(
        &self,
        user_id: Uuid,
        patch: &ProfilePatch,
    ) -> Result<Profile, Error> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        let exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE id = $1)")
            .bind(user_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(db)?;
        if !exists {
            return Err(Error::NotFound);
        }
        if let Some(display_name) = &patch.display_name {
            sqlx::query("UPDATE users SET display_name = $2 WHERE id = $1")
                .bind(user_id)
                .bind(display_name)
                .execute(&mut *tx)
                .await
                .map_err(db)?;
        }
        if let Some(locale) = &patch.locale {
            sqlx::query("UPDATE users SET locale = $2 WHERE id = $1")
                .bind(user_id)
                .bind(locale)
                .execute(&mut *tx)
                .await
                .map_err(db)?;
        }
        if let Some(age_band) = &patch.age_band {
            sqlx::query("UPDATE profiles SET age_band = $2 WHERE user_id = $1")
                .bind(user_id)
                .bind(age_band)
                .execute(&mut *tx)
                .await
                .map_err(db)?;
        }
        if let Some(gender) = &patch.gender {
            sqlx::query("UPDATE profiles SET gender = $2 WHERE user_id = $1")
                .bind(user_id)
                .bind(gender)
                .execute(&mut *tx)
                .await
                .map_err(db)?;
        }
        if let Some(mobility) = &patch.mobility {
            sqlx::query("UPDATE profiles SET mobility = $2 WHERE user_id = $1")
                .bind(user_id)
                .bind(mobility)
                .execute(&mut *tx)
                .await
                .map_err(db)?;
        }
        if let Some(sportiness) = patch.sportiness {
            sqlx::query("UPDATE profiles SET sportiness = $2 WHERE user_id = $1")
                .bind(user_id)
                .bind(sportiness)
                .execute(&mut *tx)
                .await
                .map_err(db)?;
        }
        if let Some(women_only) = patch.women_only {
            sqlx::query("UPDATE profiles SET women_only = $2 WHERE user_id = $1")
                .bind(user_id)
                .bind(women_only)
                .execute(&mut *tx)
                .await
                .map_err(db)?;
        }
        if let Some(window) = patch.time_window {
            sqlx::query(
                "UPDATE profiles SET start_minute = $2, end_minute = $3 WHERE user_id = $1",
            )
            .bind(user_id)
            .bind(i16::try_from(window.start_minute).map_err(|_| Error::Invalid)?)
            .bind(i16::try_from(window.end_minute).map_err(|_| Error::Invalid)?)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        }
        if let Some(bio) = &patch.bio {
            sqlx::query("UPDATE profiles SET bio = $2 WHERE user_id = $1")
                .bind(user_id)
                .bind(bio)
                .execute(&mut *tx)
                .await
                .map_err(db)?;
        }
        if let Some(tag) = &patch.like_tag {
            upsert_tag(&mut tx, user_id, tag, "like").await?;
        }
        if let Some(tag) = &patch.dislike_tag {
            upsert_tag(&mut tx, user_id, tag, "dislike").await?;
        }
        tx.commit().await.map_err(db)?;
        self.profile(user_id).await
    }

    pub async fn memories(&self, user_id: Uuid) -> Result<Vec<Memory>, Error> {
        let rows: Vec<MemoryRow> = sqlx::query_as(
            r#"
            SELECT id, durability, key, value, quote, confidence, confirmed
            FROM memories
            WHERE user_id = $1
            ORDER BY key, id
            "#,
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        rows.into_iter().map(memory_from_row).collect()
    }

    pub async fn remember(&self, user_id: Uuid, memory: NewMemory) -> Result<Memory, Error> {
        let row: MemoryRow = sqlx::query_as(
            r#"
            INSERT INTO memories (user_id, durability, key, value, quote, confidence)
            VALUES ($1, $2, $3, $4, $5, $6)
            RETURNING id, durability, key, value, quote, confidence, confirmed
            "#,
        )
        .bind(user_id)
        .bind(durability_db(memory.durability))
        .bind(&memory.key)
        .bind(&memory.value)
        .bind(memory.quote.as_deref())
        .bind(memory.confidence)
        .fetch_one(&self.pool)
        .await
        .map_err(db)?;
        memory_from_row(row)
    }

    pub async fn forget(&self, user_id: Uuid, topic: &str) -> Result<ForgetResult, Error> {
        let topic = topic.trim();
        if topic.is_empty() {
            return Err(Error::EmptyTopic);
        }
        let mut tx = self.pool.begin().await.map_err(db)?;
        let memories: Vec<MemoryRow> = sqlx::query_as(
            r#"
            WITH removed AS (
                DELETE FROM memories
                WHERE user_id = $1
                  AND (
                      strpos(lower(key), lower($2)) > 0
                      OR strpos(lower(value), lower($2)) > 0
                      OR strpos(lower(coalesce(quote, '')), lower($2)) > 0
                  )
                RETURNING id, durability, key, value, quote, confidence, confirmed
            )
            SELECT id, durability, key, value, quote, confidence, confirmed
            FROM removed
            ORDER BY key, id
            "#,
        )
        .bind(user_id)
        .bind(topic)
        .fetch_all(&mut *tx)
        .await
        .map_err(db)?;
        let tags: Vec<String> = sqlx::query_scalar(
            r#"
            WITH removed AS (
                DELETE FROM profile_tags
                WHERE user_id = $1 AND strpos(lower(tag), lower($2)) > 0
                RETURNING tag
            )
            SELECT tag FROM removed ORDER BY tag
            "#,
        )
        .bind(user_id)
        .bind(topic)
        .fetch_all(&mut *tx)
        .await
        .map_err(db)?;
        tx.commit().await.map_err(db)?;
        Ok(ForgetResult {
            memories: memories
                .into_iter()
                .map(memory_from_row)
                .collect::<Result<_, _>>()?,
            tags,
        })
    }

    pub async fn create_event(&self, host_id: Uuid, event: NewEvent) -> Result<Event, Error> {
        let kind = place_kind_db(event.place_kind)?;
        let mut tx = self.pool.begin().await.map_err(db)?;
        let place_id: Uuid = sqlx::query_scalar(
            "INSERT INTO places (name, kind, lat, lon) VALUES ($1, $2, $3, $4) RETURNING id",
        )
        .bind(&event.place_name)
        .bind(kind)
        .bind(event.latitude)
        .bind(event.longitude)
        .fetch_one(&mut *tx)
        .await
        .map_err(db)?;
        let event_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO events (
                host_id, place_id, title, emoji, description, starts_at,
                capacity, activity_tags, women_only
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            RETURNING id
            "#,
        )
        .bind(host_id)
        .bind(place_id)
        .bind(&event.title)
        .bind(&event.emoji)
        .bind(event.description.as_deref())
        .bind(event.starts_at)
        .bind(event.capacity)
        .bind(&event.activity_tags)
        .bind(event.women_only)
        .fetch_one(&mut *tx)
        .await
        .map_err(db)?;
        tx.commit().await.map_err(db)?;
        self.event(event_id).await
    }

    pub async fn event(&self, id: Uuid) -> Result<Event, Error> {
        let row: EventRow = sqlx::query_as(&event_sql("", "WHERE e.id = $1"))
            .bind(id)
            .fetch_one(&self.pool)
            .await
            .map_err(db)?;
        event_from_row(row)
    }

    pub async fn nearby_candidates(
        &self,
        origin: rank::LatLng,
        radius_m: f64,
        bounds: Option<rank::BBox>,
        now: DateTime<Utc>,
    ) -> Result<Vec<rank::Candidate>, Error> {
        let use_bounds = bounds.is_some();
        let bounds = bounds.unwrap_or(rank::BBox {
            west: 0.0,
            south: 0.0,
            east: 0.0,
            north: 0.0,
        });
        let sql = event_sql(
            "",
            &format!(
                r#"
                WHERE e.status = 'scheduled'
                  AND e.starts_at + interval '24 hours' > $9
                  AND p.kind IN {PUBLIC_KINDS}
                  AND (
                      ($4::bool AND p.lon >= $5 AND p.lon <= $7 AND p.lat >= $6 AND p.lat <= $8)
                      OR (
                          NOT $4::bool
                          AND (2 * {EARTH_RADIUS_M} * asin(sqrt(least(1.0,
                              power(sin(radians(p.lat - $1) / 2), 2)
                              + cos(radians($1)) * cos(radians(p.lat))
                                * power(sin(radians(p.lon - $2) / 2), 2)
                          )))) <= $3
                      )
                  )
                ORDER BY e.starts_at, e.id
                "#
            ),
        );
        let rows: Vec<EventRow> = sqlx::query_as(&sql)
            .bind(origin.lat)
            .bind(origin.lng)
            .bind(radius_m)
            .bind(use_bounds)
            .bind(bounds.west)
            .bind(bounds.south)
            .bind(bounds.east)
            .bind(bounds.north)
            .bind(now)
            .fetch_all(&self.pool)
            .await
            .map_err(db)?;
        let events = rows
            .into_iter()
            .map(event_from_row)
            .collect::<Result<Vec<_>, _>>()?;
        let attendees = self.going_attendees(&events).await?;
        Ok(events
            .into_iter()
            .map(|event| {
                let attendees = attendees.get(&event.id).cloned().unwrap_or_default();
                candidate(event, attendees)
            })
            .collect())
    }

    pub async fn my_events(&self, user_id: Uuid, now: DateTime<Utc>) -> Result<Vec<Event>, Error> {
        let sql = event_sql(
            "JOIN attendances mine ON mine.event_id = e.id",
            r#"
            WHERE mine.user_id = $1
              AND mine.status = 'going'
              AND e.status = 'scheduled'
              AND e.starts_at + interval '24 hours' > $2
            ORDER BY e.starts_at, e.id
            "#,
        );
        let rows: Vec<EventRow> = sqlx::query_as(&sql)
            .bind(user_id)
            .bind(now)
            .fetch_all(&self.pool)
            .await
            .map_err(db)?;
        rows.into_iter().map(event_from_row).collect()
    }

    pub async fn join_event(&self, event_id: Uuid, user_id: Uuid) -> Result<Attendance, Error> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        let locked: Option<(Option<i32>,)> =
            sqlx::query_as("SELECT capacity FROM events WHERE id = $1 FOR UPDATE")
                .bind(event_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(db)?;
        let Some((capacity,)) = locked else {
            return Err(Error::NotFound);
        };
        let status: Option<String> = sqlx::query_scalar(
            "SELECT status FROM attendances WHERE event_id = $1 AND user_id = $2",
        )
        .bind(event_id)
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db)?;
        if status.as_deref() != Some("going") {
            if let Some(limit) = capacity {
                let going: i64 = sqlx::query_scalar(
                    "SELECT count(*)::bigint FROM attendances WHERE event_id = $1 AND status = 'going'",
                )
                .bind(event_id)
                .fetch_one(&mut *tx)
                .await
                .map_err(db)?;
                if going >= i64::from(limit) {
                    return Err(Error::Capacity);
                }
            }
        }
        let attendance = attendance_row(
            sqlx::query_as(
                r#"
                INSERT INTO attendances (event_id, user_id, status)
                VALUES ($1, $2, 'going')
                ON CONFLICT (event_id, user_id) DO UPDATE SET status = 'going'
                RETURNING event_id, user_id, status
                "#,
            )
            .bind(event_id)
            .bind(user_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(db)?,
        )?;
        tx.commit().await.map_err(db)?;
        Ok(attendance)
    }

    pub async fn cancel_attendance(
        &self,
        event_id: Uuid,
        user_id: Uuid,
    ) -> Result<Attendance, Error> {
        self.set_attendance(event_id, user_id, "cancelled").await
    }

    pub async fn complete_attendance(
        &self,
        event_id: Uuid,
        user_id: Uuid,
    ) -> Result<Attendance, Error> {
        self.set_attendance(event_id, user_id, "completed").await
    }

    pub async fn people(
        &self,
        viewer: Uuid,
        origin: rank::LatLng,
    ) -> Result<Vec<rank::Person>, Error> {
        let _ = origin;
        let rows: Vec<PersonRow> = sqlx::query_as(&format!(
            r#"
            SELECT u.id,
                   u.display_name AS first_name,
                   pr.age_band,
                   pr.gender,
                   pr.sportiness,
                   pr.women_only,
                   pr.start_minute,
                   pr.end_minute,
                   pr.embedding,
                   COALESCE(
                       (SELECT array_agg(tag ORDER BY tag)
                        FROM profile_tags t
                        WHERE t.user_id = u.id AND t.polarity = 'like'),
                       '{{}}'::text[]
                   ) AS tags,
                   loc.lat,
                   loc.lon
            FROM users u
            JOIN profiles pr ON pr.user_id = u.id
            LEFT JOIN LATERAL (
                SELECT pl.lat, pl.lon
                FROM events e
                JOIN places pl ON pl.id = e.place_id
                LEFT JOIN attendances a
                  ON a.event_id = e.id AND a.user_id = u.id AND a.status = 'going'
                WHERE e.status = 'scheduled'
                  AND e.starts_at + interval '24 hours' > now()
                  AND pl.kind IN {PUBLIC_KINDS}
                  AND (e.host_id = u.id OR a.user_id IS NOT NULL)
                ORDER BY (a.user_id IS NOT NULL) DESC, e.starts_at
                LIMIT 1
            ) loc ON true
            WHERE u.id <> $1
            ORDER BY u.display_name, u.id
            "#
        ))
        .bind(viewer)
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        rows.into_iter().map(person_from_row).collect()
    }

    pub async fn conversation(&self, user_id: Uuid) -> Result<Conversation, Error> {
        sqlx::query_as(
            "SELECT user_id, summary, summary_through FROM conversations WHERE user_id = $1",
        )
        .bind(user_id)
        .fetch_one(&self.pool)
        .await
        .map_err(db)
    }

    pub async fn set_summary(
        &self,
        user_id: Uuid,
        summary: &str,
        through_message: Uuid,
    ) -> Result<(), Error> {
        let updated = sqlx::query(
            r#"
            UPDATE conversations
            SET summary = $2, summary_through = $3
            WHERE user_id = $1
              AND EXISTS (
                  SELECT 1 FROM messages WHERE id = $3 AND user_id = $1
              )
            "#,
        )
        .bind(user_id)
        .bind(summary)
        .bind(through_message)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        ensure_row(updated.rows_affected())
    }

    pub async fn recent_turns(
        &self,
        user_id: Uuid,
        user_turns: i64,
    ) -> Result<Vec<ChatMessage>, Error> {
        if user_turns <= 0 {
            return Ok(Vec::new());
        }
        let rows: Vec<MessageRow> = sqlx::query_as(
            r#"
            WITH ranked AS (
                SELECT id, created_at
                FROM messages
                WHERE user_id = $1 AND role = 'user'
                ORDER BY created_at DESC, id DESC
                LIMIT $2
            ),
            cutoff AS (
                SELECT created_at, id
                FROM ranked
                ORDER BY created_at ASC, id ASC
                LIMIT 1
            )
            SELECT m.id, m.user_id, m.role, m.body, m.created_at
            FROM messages m
            CROSS JOIN cutoff c
            WHERE m.user_id = $1
              AND (m.created_at, m.id) >= (c.created_at, c.id)
            ORDER BY m.created_at ASC, m.id ASC
            "#,
        )
        .bind(user_id)
        .bind(user_turns)
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        rows.into_iter().map(chat_message).collect()
    }

    pub async fn messages_before(
        &self,
        user_id: Uuid,
        before: Option<Uuid>,
        limit: i64,
    ) -> Result<Vec<ChatMessage>, Error> {
        if limit <= 0 {
            return Ok(Vec::new());
        }
        let rows: Vec<MessageRow> = sqlx::query_as(
            r#"
            SELECT id, user_id, role, body, created_at
            FROM (
                SELECT id, user_id, role, body, created_at
                FROM messages
                WHERE user_id = $1
                  AND (
                      $2::uuid IS NULL
                      OR (created_at, id) < (
                          SELECT created_at, id
                          FROM messages
                          WHERE id = $2 AND user_id = $1
                      )
                  )
                ORDER BY created_at DESC, id DESC
                LIMIT $3
            ) AS page
            ORDER BY created_at ASC, id ASC
            "#,
        )
        .bind(user_id)
        .bind(before)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        rows.into_iter().map(chat_message).collect()
    }

    pub async fn search_messages(
        &self,
        user_id: Uuid,
        query: &str,
    ) -> Result<Vec<ChatMessage>, Error> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let rows: Vec<MessageRow> = sqlx::query_as(
            r#"
            SELECT id, user_id, role, body, created_at
            FROM messages
            WHERE user_id = $1 AND body ILIKE $2 ESCAPE '\'
            ORDER BY word_similarity($3, body) DESC, created_at ASC, id ASC
            "#,
        )
        .bind(user_id)
        .bind(like_contains(query))
        .bind(query)
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        rows.into_iter().map(chat_message).collect()
    }

    pub async fn count_user_messages(&self, user_id: Uuid) -> Result<i64, Error> {
        sqlx::query_scalar(
            "SELECT count(*)::bigint FROM messages WHERE user_id = $1 AND role = 'user'",
        )
        .bind(user_id)
        .fetch_one(&self.pool)
        .await
        .map_err(db)
    }

    pub async fn insert_message(
        &self,
        user_id: Uuid,
        role: ChatRole,
        body: &str,
    ) -> Result<ChatMessage, Error> {
        let row: MessageRow = sqlx::query_as(
            r#"
            INSERT INTO messages (user_id, role, body)
            VALUES ($1, $2, $3)
            RETURNING id, user_id, role, body, created_at
            "#,
        )
        .bind(user_id)
        .bind(chat_role_db(role))
        .bind(body)
        .fetch_one(&self.pool)
        .await
        .map_err(db)?;
        chat_message(row)
    }

    pub async fn insert_turn(&self, user_id: Uuid) -> Result<Turn, Error> {
        let row: TurnRow = sqlx::query_as(
            r#"
            INSERT INTO turns (user_id)
            VALUES ($1)
            RETURNING id, user_id, status, user_text, reply_text, audio_path, checkpoint, error
            "#,
        )
        .bind(user_id)
        .fetch_one(&self.pool)
        .await
        .map_err(db)?;
        turn_from_row(row)
    }

    pub async fn turn(&self, user_id: Uuid, turn_id: Uuid) -> Result<Turn, Error> {
        let row: TurnRow = sqlx::query_as(
            r#"
            SELECT id, user_id, status, user_text, reply_text, audio_path, checkpoint, error
            FROM turns
            WHERE id = $1 AND user_id = $2
            "#,
        )
        .bind(turn_id)
        .bind(user_id)
        .fetch_one(&self.pool)
        .await
        .map_err(db)?;
        turn_from_row(row)
    }

    pub async fn set_turn_user_text(&self, turn_id: Uuid, text: &str) -> Result<(), Error> {
        let updated = sqlx::query("UPDATE turns SET user_text = $2 WHERE id = $1")
            .bind(turn_id)
            .bind(text)
            .execute(&self.pool)
            .await
            .map_err(db)?;
        ensure_row(updated.rows_affected())
    }

    pub async fn set_checkpoint(&self, turn_id: Uuid, checkpoint: Value) -> Result<(), Error> {
        let updated = sqlx::query("UPDATE turns SET checkpoint = $2 WHERE id = $1")
            .bind(turn_id)
            .bind(checkpoint)
            .execute(&self.pool)
            .await
            .map_err(db)?;
        ensure_row(updated.rows_affected())
    }

    pub async fn finish_turn(
        &self,
        turn_id: Uuid,
        reply_text: &str,
        audio_path: Option<&str>,
    ) -> Result<(), Error> {
        let updated = sqlx::query(
            "UPDATE turns SET status = 'done', reply_text = $2, audio_path = $3 WHERE id = $1",
        )
        .bind(turn_id)
        .bind(reply_text)
        .bind(audio_path)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        ensure_row(updated.rows_affected())
    }

    pub async fn fail_turn(&self, turn_id: Uuid, message: &str) -> Result<(), Error> {
        let updated = sqlx::query("UPDATE turns SET status = 'failed', error = $2 WHERE id = $1")
            .bind(turn_id)
            .bind(message)
            .execute(&self.pool)
            .await
            .map_err(db)?;
        ensure_row(updated.rows_affected())
    }

    pub async fn running_turns(&self) -> Result<Vec<Turn>, Error> {
        let rows: Vec<TurnRow> = sqlx::query_as(
            r#"
            SELECT id, user_id, status, user_text, reply_text, audio_path, checkpoint, error
            FROM turns
            WHERE status = 'running'
            ORDER BY id
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        rows.into_iter().map(turn_from_row).collect()
    }

    pub async fn append_stream(
        &self,
        user_id: Uuid,
        turn_id: Uuid,
        kind: &str,
        payload: Value,
    ) -> Result<i64, Error> {
        sqlx::query_scalar(
            r#"
            INSERT INTO stream_events (user_id, turn_id, kind, payload)
            VALUES ($1, $2, $3, $4)
            RETURNING id
            "#,
        )
        .bind(user_id)
        .bind(turn_id)
        .bind(kind)
        .bind(payload)
        .fetch_one(&self.pool)
        .await
        .map_err(db)
    }

    pub async fn stream_after(
        &self,
        user_id: Uuid,
        after_id: i64,
    ) -> Result<Vec<StreamEvent>, Error> {
        sqlx::query_as(
            r#"
            SELECT id, user_id, turn_id, kind, payload, created_at
            FROM stream_events
            WHERE user_id = $1 AND id > $2
            ORDER BY id
            "#,
        )
        .bind(user_id)
        .bind(after_id)
        .fetch_all(&self.pool)
        .await
        .map_err(db)
    }

    async fn set_attendance(
        &self,
        event_id: Uuid,
        user_id: Uuid,
        status: &str,
    ) -> Result<Attendance, Error> {
        let row: Option<(Uuid, Uuid, String)> = sqlx::query_as(
            r#"
            UPDATE attendances
            SET status = $3
            WHERE event_id = $1 AND user_id = $2
            RETURNING event_id, user_id, status
            "#,
        )
        .bind(event_id)
        .bind(user_id)
        .bind(status)
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        let Some(row) = row else {
            return Err(Error::NotFound);
        };
        attendance_row(row)
    }

    async fn going_attendees(
        &self,
        events: &[Event],
    ) -> Result<HashMap<Uuid, Vec<rank::Attendee>>, Error> {
        let ids: Vec<Uuid> = events.iter().map(|event| event.id).collect();
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let rows: Vec<AttendeeRow> = sqlx::query_as(
            r#"
            SELECT a.event_id,
                   pr.age_band,
                   pr.sportiness,
                   pr.embedding,
                   COALESCE(
                       (SELECT array_agg(tag ORDER BY tag)
                        FROM profile_tags t
                        WHERE t.user_id = a.user_id AND t.polarity = 'like'),
                       '{}'::text[]
                   ) AS tags
            FROM attendances a
            LEFT JOIN profiles pr ON pr.user_id = a.user_id
            WHERE a.status = 'going' AND a.event_id = ANY($1)
            ORDER BY a.event_id, a.user_id
            "#,
        )
        .bind(&ids)
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        let mut grouped: HashMap<Uuid, Vec<rank::Attendee>> = HashMap::new();
        for row in rows {
            grouped
                .entry(row.event_id)
                .or_default()
                .push(rank::Attendee {
                    age_band: row.age_band,
                    sportiness: row.sportiness,
                    tags: row.tags,
                    embedding: row.embedding.map(|vector| vector.to_vec()),
                    complements: false,
                });
        }
        Ok(grouped)
    }
}

const PUBLIC_KINDS: &str = "('cafe', 'park', 'hall', 'square', 'other_public')";

const EVENT_COLUMNS: &str = r#"
    e.id,
    e.host_id,
    u.display_name AS host_name,
    e.title,
    e.emoji,
    e.description,
    e.starts_at,
    e.capacity,
    e.activity_tags,
    e.promoted,
    e.status,
    e.women_only,
    p.name AS place_name,
    p.kind AS place_kind,
    p.lat AS latitude,
    p.lon AS longitude,
    (SELECT count(*)::bigint FROM attendances a WHERE a.event_id = e.id AND a.status = 'going') AS signed_count
"#;

fn event_sql(extra_from: &str, filter: &str) -> String {
    format!(
        "SELECT {EVENT_COLUMNS}
         FROM events e
         JOIN places p ON p.id = e.place_id
         JOIN users u ON u.id = e.host_id
         {extra_from}
         {filter}"
    )
}

fn profile_sql() -> String {
    r#"
    SELECT p.user_id,
           p.age_band,
           p.gender,
           p.mobility,
           p.sportiness,
           p.bio,
           p.women_only,
           p.start_minute,
           p.end_minute,
           p.embedding,
           p.embedding_model,
           COALESCE(
               (SELECT array_agg(tag ORDER BY tag)
                FROM profile_tags t
                WHERE t.user_id = p.user_id AND t.polarity = 'like'),
               '{}'::text[]
           ) AS likes,
           COALESCE(
               (SELECT array_agg(tag ORDER BY tag)
                FROM profile_tags t
                WHERE t.user_id = p.user_id AND t.polarity = 'dislike'),
               '{}'::text[]
           ) AS dislikes
    FROM profiles p
    WHERE p.user_id = $1
    "#
    .to_string()
}

/// 001 already created the ingest tables on databases that existed before this store.
/// Record that version so those CREATE statements are not run again.
async fn migrate(pool: &PgPool) -> Result<(), Error> {
    record_existing_ingest_migration(pool).await?;
    let migrator =
        sqlx::migrate::Migrator::new(Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations"))
            .await
            .map_err(|err| Error::Database(redact(&err.to_string())))?;
    migrator
        .run(pool)
        .await
        .map_err(|err| Error::Database(redact(&err.to_string())))?;
    Ok(())
}

async fn record_existing_ingest_migration(pool: &PgPool) -> Result<(), Error> {
    let exists: bool = sqlx::query_scalar("SELECT to_regclass('public.innovations') IS NOT NULL")
        .fetch_one(pool)
        .await
        .map_err(db)?;
    if !exists {
        return Ok(());
    }
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS _sqlx_migrations (
            version BIGINT PRIMARY KEY,
            description TEXT NOT NULL,
            installed_on TIMESTAMPTZ NOT NULL DEFAULT now(),
            success BOOLEAN NOT NULL,
            checksum BYTEA NOT NULL,
            execution_time BIGINT NOT NULL
        )
        "#,
    )
    .execute(pool)
    .await
    .map_err(db)?;
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations/001_init.sql");
    let sql = std::fs::read_to_string(&path).map_err(|err| Error::Database(err.to_string()))?;
    let migration = sqlx::migrate::Migration::new(
        1,
        std::borrow::Cow::Borrowed("init"),
        sqlx::migrate::MigrationType::Simple,
        std::borrow::Cow::Owned(sql),
        false,
    );
    sqlx::query(
        r#"
        INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time)
        VALUES (1, 'init', TRUE, $1, 0)
        ON CONFLICT (version) DO NOTHING
        "#,
    )
    .bind(migration.checksum.as_ref())
    .execute(pool)
    .await
    .map_err(db)?;
    Ok(())
}

fn db(err: sqlx::Error) -> Error {
    match &err {
        sqlx::Error::RowNotFound => Error::NotFound,
        sqlx::Error::Database(inner) if inner.code().as_deref() == Some("23514") => Error::Invalid,
        _ => Error::Database(redact(&err.to_string())),
    }
}

fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(scheme) = rest.find("://") {
        out.push_str(&rest[..scheme + 3]);
        rest = &rest[scheme + 3..];
        let end = rest.find(['/', '?', ' ']).unwrap_or(rest.len());
        let userinfo = &rest[..end];
        if let Some(colon) = userinfo.find(':') {
            if userinfo[colon + 1..].contains('@') {
                let at = userinfo.find('@').unwrap_or(userinfo.len());
                out.push_str(&userinfo[..colon]);
                out.push_str(":***");
                out.push_str(&userinfo[at..]);
                rest = &rest[end..];
                continue;
            }
        }
        out.push_str(userinfo);
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

fn ensure_row(affected: u64) -> Result<(), Error> {
    if affected == 0 {
        Err(Error::NotFound)
    } else {
        Ok(())
    }
}

async fn upsert_tag(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    tag: &str,
    polarity: &str,
) -> Result<(), Error> {
    sqlx::query(
        r#"
        INSERT INTO profile_tags (user_id, tag, polarity)
        VALUES ($1, $2, $3)
        ON CONFLICT (user_id, tag) DO UPDATE SET polarity = EXCLUDED.polarity
        "#,
    )
    .bind(user_id)
    .bind(tag)
    .bind(polarity)
    .execute(&mut **tx)
    .await
    .map_err(db)?;
    Ok(())
}

fn like_contains(query: &str) -> String {
    let mut pattern = String::from("%");
    for ch in query.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            pattern.push('\\');
        }
        pattern.push(ch);
    }
    pattern.push('%');
    pattern
}

fn durability_db(durability: Durability) -> &'static str {
    match durability {
        Durability::LongTerm => "long_term",
        Durability::ShortTerm => "short_term",
    }
}

fn durability_from(value: &str) -> Result<Durability, Error> {
    match value {
        "long_term" => Ok(Durability::LongTerm),
        "short_term" => Ok(Durability::ShortTerm),
        _ => Err(Error::Invalid),
    }
}

fn place_kind_db(kind: PlaceKind) -> Result<&'static str, Error> {
    Ok(match kind {
        PlaceKind::Cafe => "cafe",
        PlaceKind::Park => "park",
        PlaceKind::Hall => "hall",
        PlaceKind::Square => "square",
        PlaceKind::OtherPublic => "other_public",
        PlaceKind::NotPublic => return Err(Error::Invalid),
    })
}

fn place_kind(value: &str) -> Result<PlaceKind, Error> {
    Ok(match value {
        "cafe" => PlaceKind::Cafe,
        "park" => PlaceKind::Park,
        "hall" => PlaceKind::Hall,
        "square" => PlaceKind::Square,
        "other_public" => PlaceKind::OtherPublic,
        _ => return Err(Error::Invalid),
    })
}

fn event_status(value: &str) -> Result<EventStatus, Error> {
    match value {
        "scheduled" => Ok(EventStatus::Scheduled),
        "cancelled" => Ok(EventStatus::Cancelled),
        _ => Err(Error::Invalid),
    }
}

fn attendance_status(value: &str) -> Result<AttendanceStatus, Error> {
    match value {
        "going" => Ok(AttendanceStatus::Going),
        "cancelled" => Ok(AttendanceStatus::Cancelled),
        "completed" => Ok(AttendanceStatus::Completed),
        _ => Err(Error::Invalid),
    }
}

fn attendance_row(row: (Uuid, Uuid, String)) -> Result<Attendance, Error> {
    Ok(Attendance {
        event_id: row.0,
        user_id: row.1,
        status: attendance_status(&row.2)?,
    })
}

fn chat_role_db(role: ChatRole) -> &'static str {
    match role {
        ChatRole::User => "user",
        ChatRole::Assistant => "assistant",
    }
}

fn chat_role(value: &str) -> Result<ChatRole, Error> {
    match value {
        "user" => Ok(ChatRole::User),
        "assistant" => Ok(ChatRole::Assistant),
        _ => Err(Error::Invalid),
    }
}

fn turn_status(value: &str) -> Result<TurnStatus, Error> {
    match value {
        "running" => Ok(TurnStatus::Running),
        "done" => Ok(TurnStatus::Done),
        "failed" => Ok(TurnStatus::Failed),
        _ => Err(Error::Invalid),
    }
}

fn time_window(start: Option<i16>, end: Option<i16>) -> Result<Option<TimeWindow>, Error> {
    match (start, end) {
        (Some(start), Some(end)) => Ok(Some(TimeWindow {
            start_minute: u16::try_from(start).map_err(|_| Error::Invalid)?,
            end_minute: u16::try_from(end).map_err(|_| Error::Invalid)?,
        })),
        _ => Ok(None),
    }
}

fn profile_from_row(row: ProfileRow) -> Result<Profile, Error> {
    Ok(Profile {
        user_id: row.user_id,
        age_band: row.age_band,
        gender: row.gender,
        mobility: row.mobility,
        sportiness: row.sportiness,
        bio: row.bio,
        likes: row.likes,
        dislikes: row.dislikes,
        women_only: row.women_only,
        time_window: time_window(row.start_minute, row.end_minute)?,
        embedding: row.embedding,
        embedding_model: row.embedding_model,
    })
}

fn memory_from_row(row: MemoryRow) -> Result<Memory, Error> {
    Ok(Memory {
        id: row.id,
        durability: durability_from(&row.durability)?,
        key: row.key,
        value: row.value,
        quote: row.quote,
        confidence: row.confidence,
        confirmed: row.confirmed,
    })
}

fn event_from_row(row: EventRow) -> Result<Event, Error> {
    Ok(Event {
        id: row.id,
        host_id: row.host_id,
        host_name: row.host_name,
        title: row.title,
        emoji: row.emoji,
        description: row.description,
        starts_at: row.starts_at,
        capacity: row.capacity,
        activity_tags: row.activity_tags,
        promoted: row.promoted,
        status: event_status(&row.status)?,
        women_only: row.women_only,
        place_name: row.place_name,
        place_kind: place_kind(&row.place_kind)?,
        latitude: row.latitude,
        longitude: row.longitude,
        signed_count: row.signed_count,
    })
}

fn candidate(event: Event, attendees: Vec<rank::Attendee>) -> rank::Candidate {
    rank::Candidate {
        id: event.id,
        title: event.title,
        emoji: event.emoji,
        description: event.description.unwrap_or_default(),
        activity_tags: event.activity_tags,
        women_only: event.women_only,
        starts_at: event.starts_at,
        capacity: event.capacity,
        promoted: event.promoted,
        status: event.status,
        place_name: event.place_name,
        place_kind: event.place_kind,
        latitude: event.latitude,
        longitude: event.longitude,
        host_name: event.host_name,
        signed_count: event.signed_count,
        embedding: None,
        attendees,
    }
}

fn person_from_row(row: PersonRow) -> Result<rank::Person, Error> {
    Ok(rank::Person {
        id: row.id,
        first_name: row.first_name,
        age_band: row.age_band,
        gender: row.gender,
        mobility: None,
        sportiness: row.sportiness,
        tags: row.tags,
        women_only: row.women_only,
        time_window: time_window(row.start_minute, row.end_minute)?,
        embedding: row.embedding.map(|vector| vector.to_vec()),
        latitude: row.lat.unwrap_or(0.0),
        longitude: row.lon.unwrap_or(0.0),
        complements: false,
    })
}

fn chat_message(row: MessageRow) -> Result<ChatMessage, Error> {
    Ok(ChatMessage {
        id: row.id,
        user_id: row.user_id,
        role: chat_role(&row.role)?,
        body: row.body,
        created_at: row.created_at,
    })
}

fn turn_from_row(row: TurnRow) -> Result<Turn, Error> {
    Ok(Turn {
        id: row.id,
        user_id: row.user_id,
        status: turn_status(&row.status)?,
        user_text: row.user_text,
        reply_text: row.reply_text,
        audio_path: row.audio_path,
        checkpoint: row.checkpoint,
        error: row.error,
    })
}

#[derive(sqlx::FromRow)]
struct ProfileRow {
    user_id: Uuid,
    age_band: Option<String>,
    gender: Option<String>,
    mobility: Option<String>,
    sportiness: Option<i16>,
    bio: Option<String>,
    women_only: bool,
    start_minute: Option<i16>,
    end_minute: Option<i16>,
    embedding: Option<pgvector::Vector>,
    embedding_model: Option<String>,
    likes: Vec<String>,
    dislikes: Vec<String>,
}

#[derive(sqlx::FromRow)]
struct MemoryRow {
    id: Uuid,
    durability: String,
    key: String,
    value: String,
    quote: Option<String>,
    confidence: Option<f32>,
    confirmed: bool,
}

#[derive(sqlx::FromRow)]
struct EventRow {
    id: Uuid,
    host_id: Uuid,
    host_name: String,
    title: String,
    emoji: String,
    description: Option<String>,
    starts_at: DateTime<Utc>,
    capacity: Option<i32>,
    activity_tags: Vec<String>,
    promoted: bool,
    status: String,
    women_only: bool,
    place_name: String,
    place_kind: String,
    latitude: f64,
    longitude: f64,
    signed_count: i64,
}

#[derive(sqlx::FromRow)]
struct AttendeeRow {
    event_id: Uuid,
    age_band: Option<String>,
    sportiness: Option<i16>,
    embedding: Option<pgvector::Vector>,
    tags: Vec<String>,
}

#[derive(sqlx::FromRow)]
struct PersonRow {
    id: Uuid,
    first_name: String,
    age_band: Option<String>,
    gender: Option<String>,
    sportiness: Option<i16>,
    women_only: bool,
    start_minute: Option<i16>,
    end_minute: Option<i16>,
    embedding: Option<pgvector::Vector>,
    tags: Vec<String>,
    lat: Option<f64>,
    lon: Option<f64>,
}

#[derive(sqlx::FromRow)]
struct MessageRow {
    id: Uuid,
    user_id: Uuid,
    role: String,
    body: String,
    created_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct TurnRow {
    id: Uuid,
    user_id: Uuid,
    status: String,
    user_text: Option<String>,
    reply_text: Option<String>,
    audio_path: Option<String>,
    checkpoint: Option<Value>,
    error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, sqlx::FromRow)]
pub struct User {
    pub id: Uuid,
    pub display_name: String,
    pub locale: String,
}

#[derive(Clone, Debug)]
pub struct Profile {
    pub user_id: Uuid,
    pub age_band: Option<String>,
    pub gender: Option<String>,
    pub mobility: Option<String>,
    pub sportiness: Option<i16>,
    pub bio: Option<String>,
    pub likes: Vec<String>,
    pub dislikes: Vec<String>,
    pub women_only: bool,
    pub time_window: Option<TimeWindow>,
    pub embedding: Option<pgvector::Vector>,
    pub embedding_model: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ProfilePatch {
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub locale: Option<String>,
    #[serde(default)]
    pub age_band: Option<String>,
    #[serde(default)]
    pub gender: Option<String>,
    #[serde(default)]
    pub mobility: Option<String>,
    #[serde(default)]
    pub sportiness: Option<i16>,
    #[serde(default)]
    pub women_only: Option<bool>,
    #[serde(default)]
    pub time_window: Option<TimeWindow>,
    #[serde(default)]
    pub like_tag: Option<String>,
    #[serde(default)]
    pub dislike_tag: Option<String>,
    #[serde(default)]
    pub bio: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Durability {
    LongTerm,
    ShortTerm,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NewMemory {
    pub durability: Durability,
    pub key: String,
    pub value: String,
    pub quote: Option<String>,
    pub confidence: Option<f32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Memory {
    pub id: Uuid,
    pub durability: Durability,
    pub key: String,
    pub value: String,
    pub quote: Option<String>,
    pub confidence: Option<f32>,
    pub confirmed: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ForgetResult {
    pub memories: Vec<Memory>,
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NewEvent {
    pub title: String,
    pub emoji: String,
    pub description: Option<String>,
    pub starts_at: DateTime<Utc>,
    pub capacity: Option<i32>,
    pub activity_tags: Vec<String>,
    pub women_only: bool,
    pub place_name: String,
    pub place_kind: PlaceKind,
    pub latitude: f64,
    pub longitude: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub id: Uuid,
    pub host_id: Uuid,
    pub host_name: String,
    pub title: String,
    pub emoji: String,
    pub description: Option<String>,
    pub starts_at: DateTime<Utc>,
    pub capacity: Option<i32>,
    pub activity_tags: Vec<String>,
    pub promoted: bool,
    pub status: EventStatus,
    pub women_only: bool,
    pub place_name: String,
    pub place_kind: PlaceKind,
    pub latitude: f64,
    pub longitude: f64,
    pub signed_count: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttendanceStatus {
    Going,
    Cancelled,
    Completed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attendance {
    pub event_id: Uuid,
    pub user_id: Uuid,
    pub status: AttendanceStatus,
}

#[derive(Clone, Debug, PartialEq, Eq, sqlx::FromRow)]
pub struct Conversation {
    pub user_id: Uuid,
    pub summary: Option<String>,
    pub summary_through: Option<Uuid>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatRole {
    User,
    Assistant,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatMessage {
    pub id: Uuid,
    pub user_id: Uuid,
    pub role: ChatRole,
    pub body: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnStatus {
    Running,
    Done,
    Failed,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Turn {
    pub id: Uuid,
    pub user_id: Uuid,
    pub status: TurnStatus,
    pub user_text: Option<String>,
    pub reply_text: Option<String>,
    pub audio_path: Option<String>,
    pub checkpoint: Option<Value>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, sqlx::FromRow)]
pub struct StreamEvent {
    pub id: i64,
    pub user_id: Uuid,
    pub turn_id: Option<Uuid>,
    pub kind: String,
    pub payload: Value,
    pub created_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn live_url() -> &'static str {
        static URL: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        URL.get_or_init(|| {
            let path = concat!(env!("CARGO_MANIFEST_DIR"), "/.env");
            let text = std::fs::read_to_string(path).expect("backend/.env");
            for line in text.lines() {
                let line = line.trim();
                if let Some(value) = line.strip_prefix("DATABASE_URL=") {
                    return value.trim_matches('"').trim_matches('\'').to_string();
                }
            }
            panic!("DATABASE_URL is not set");
        })
    }

    fn database_name(url: &str) -> &str {
        let base = url.split('?').next().unwrap_or(url);
        base.rsplit('/').next().unwrap_or("")
    }

    fn with_database(url: &str, name: &str) -> String {
        let (base, query) = match url.split_once('?') {
            Some((base, query)) => (base, Some(query)),
            None => (url, None),
        };
        let slash = base.rfind('/').expect("database url path");
        let mut out = format!("{}{name}", &base[..=slash]);
        if let Some(query) = query {
            out.push('?');
            out.push_str(query);
        }
        out
    }

    async fn quiet_pool(url: &str) -> PgPool {
        PgPool::connect(url)
            .await
            .unwrap_or_else(|err| panic!("connect: {}", redact(&err.to_string())))
    }

    fn test_database_url() -> String {
        assert_eq!(database_name(live_url()), "styrta");
        let url = with_database(live_url(), "styrta_schema_test");
        assert_eq!(database_name(&url), "styrta_schema_test");
        url
    }

    /// Each `#[tokio::test]` owns a runtime. A pool opened on another test's runtime
    /// cannot acquire connections, so every test connects for itself.
    async fn schema_store() -> Store {
        static READY: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();
        READY
            .get_or_init(|| async {
                let admin = quiet_pool(live_url()).await;
                sqlx::query(
                    "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = 'styrta_schema_test' AND pid <> pg_backend_pid()",
                )
                .execute(&admin)
                .await
                .unwrap_or_else(|err| panic!("terminate: {}", redact(&err.to_string())));
                sqlx::raw_sql("DROP DATABASE IF EXISTS styrta_schema_test")
                    .execute(&admin)
                    .await
                    .unwrap_or_else(|err| {
                        panic!("drop test database: {}", redact(&err.to_string()))
                    });
                sqlx::raw_sql("CREATE DATABASE styrta_schema_test")
                    .execute(&admin)
                    .await
                    .unwrap_or_else(|err| {
                        panic!("create test database: {}", redact(&err.to_string()))
                    });
                admin.close().await;
                let url = test_database_url();
                let store = Store::connect(&url)
                    .await
                    .unwrap_or_else(|err| panic!("migrate test database: {err}"));
                let again = Store::connect(&url)
                    .await
                    .unwrap_or_else(|err| panic!("second migrate: {err}"));
                let versions: Vec<i64> = sqlx::query_scalar(
                    "SELECT version FROM _sqlx_migrations ORDER BY version",
                )
                .fetch_all(store.pool())
                .await
                .unwrap_or_else(|err| panic!("versions: {}", redact(&err.to_string())));
                assert_eq!(versions, vec![1, 2]);
                store.pool().close().await;
                again.pool().close().await;
            })
            .await;
        Store::connect(&test_database_url())
            .await
            .unwrap_or_else(|err| panic!("connect test database: {err}"))
    }

    fn soon(hours: i64) -> DateTime<Utc> {
        Utc::now() + Duration::hours(hours)
    }

    fn new_event(
        title: &str,
        lat: f64,
        lon: f64,
        capacity: Option<i32>,
        starts_at: DateTime<Utc>,
    ) -> NewEvent {
        NewEvent {
            title: title.to_string(),
            emoji: "☕".to_string(),
            description: None,
            starts_at,
            capacity,
            activity_tags: Vec::new(),
            women_only: false,
            place_name: title.to_string(),
            place_kind: PlaceKind::Cafe,
            latitude: lat,
            longitude: lon,
        }
    }

    #[tokio::test]
    async fn create_user_opens_profile_and_conversation() {
        let store = schema_store().await;
        let user = store
            .create_user("Anna", "pl")
            .await
            .unwrap_or_else(|err| panic!("{err}"));
        assert_eq!(user.display_name, "Anna");
        assert_eq!(user.locale, "pl");
        let loaded = store.user(user.id).await.unwrap();
        assert_eq!(loaded, user);
        let profile = store.profile(user.id).await.unwrap();
        assert_eq!(profile.user_id, user.id);
        assert!(profile.likes.is_empty());
        assert!(profile.dislikes.is_empty());
        assert!(!profile.women_only);
        assert!(profile.embedding.is_none());
        assert!(profile.mobility.is_none());
        let conversation = store.conversation(user.id).await.unwrap();
        assert_eq!(conversation.user_id, user.id);
        assert!(conversation.summary.is_none());
        assert!(conversation.summary_through.is_none());
    }

    #[tokio::test]
    async fn forget_topic_removes_tennis_and_keeps_coffee() {
        let store = schema_store().await;
        let user = store.create_user("Basia", "pl").await.unwrap();
        store
            .remember(
                user.id,
                NewMemory {
                    durability: Durability::LongTerm,
                    key: "sport".to_string(),
                    value: "plays Tennis on thursdays".to_string(),
                    quote: Some("I love tennis".to_string()),
                    confidence: Some(0.8),
                },
            )
            .await
            .unwrap();
        store
            .remember(
                user.id,
                NewMemory {
                    durability: Durability::ShortTerm,
                    key: "drink".to_string(),
                    value: "likes coffee".to_string(),
                    quote: None,
                    confidence: None,
                },
            )
            .await
            .unwrap();
        store
            .apply_profile(
                user.id,
                &ProfilePatch {
                    like_tag: Some("tennis".to_string()),
                    ..ProfilePatch::default()
                },
            )
            .await
            .unwrap();
        store
            .apply_profile(
                user.id,
                &ProfilePatch {
                    like_tag: Some("coffee".to_string()),
                    ..ProfilePatch::default()
                },
            )
            .await
            .unwrap();
        let empty = store.forget(user.id, "  ").await.unwrap_err();
        assert_eq!(empty, Error::EmptyTopic);
        assert_eq!(store.memories(user.id).await.unwrap().len(), 2);

        let removed = store.forget(user.id, "tennis").await.unwrap();
        assert_eq!(removed.memories.len(), 1);
        assert_eq!(removed.memories[0].key, "sport");
        assert_eq!(removed.tags, vec!["tennis".to_string()]);
        let left = store.memories(user.id).await.unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].value, "likes coffee");
        let profile = store.profile(user.id).await.unwrap();
        assert_eq!(profile.likes, vec!["coffee".to_string()]);
        assert!(profile.dislikes.is_empty());
    }

    #[tokio::test]
    async fn join_second_join_capacity_cancel_complete_and_my_events() {
        let store = schema_store().await;
        let host = store.create_user("Host", "pl").await.unwrap();
        let guest = store.create_user("Guest", "pl").await.unwrap();
        let open = store
            .create_event(host.id, new_event("open", 1.0, 1.0, None, soon(4)))
            .await
            .unwrap();
        let first = store.join_event(open.id, guest.id).await.unwrap();
        let second = store.join_event(open.id, guest.id).await.unwrap();
        assert_eq!(first.status, AttendanceStatus::Going);
        assert_eq!(second.status, AttendanceStatus::Going);
        assert_eq!(store.event(open.id).await.unwrap().signed_count, 1);

        let limited = store
            .create_event(host.id, new_event("limited", 1.1, 1.1, Some(1), soon(4)))
            .await
            .unwrap();
        store.join_event(limited.id, host.id).await.unwrap();
        let full = store.join_event(limited.id, guest.id).await.unwrap_err();
        assert_eq!(full, Error::Capacity);
        assert_eq!(store.event(limited.id).await.unwrap().signed_count, 1);
        let guest_rows: i64 = sqlx::query_scalar(
            "SELECT count(*)::bigint FROM attendances WHERE event_id = $1 AND user_id = $2",
        )
        .bind(limited.id)
        .bind(guest.id)
        .fetch_one(store.pool())
        .await
        .unwrap();
        assert_eq!(guest_rows, 0);

        let cancelled = store.cancel_attendance(open.id, guest.id).await.unwrap();
        assert_eq!(cancelled.status, AttendanceStatus::Cancelled);
        let finished = store
            .create_event(host.id, new_event("finished", 1.2, 1.2, None, soon(4)))
            .await
            .unwrap();
        store.join_event(finished.id, guest.id).await.unwrap();
        let completed = store
            .complete_attendance(finished.id, guest.id)
            .await
            .unwrap();
        assert_eq!(completed.status, AttendanceStatus::Completed);
        let expired = store
            .create_event(
                host.id,
                new_event("expired", 1.3, 1.3, None, Utc::now() - Duration::hours(48)),
            )
            .await
            .unwrap();
        store.join_event(expired.id, guest.id).await.unwrap();

        let now = Utc::now();
        let guest_events = store.my_events(guest.id, now).await.unwrap();
        assert!(guest_events.iter().all(|event| event.id != open.id));
        assert!(guest_events.iter().all(|event| event.id != finished.id));
        assert!(guest_events.iter().all(|event| event.id != expired.id));
        assert!(guest_events.is_empty());
        let host_events = store.my_events(host.id, now).await.unwrap();
        assert!(host_events.iter().any(|event| event.id == limited.id));
        assert!(host_events.iter().all(|event| event.id != expired.id));
    }

    #[tokio::test]
    async fn stream_after_is_user_scoped() {
        let store = schema_store().await;
        let anna = store.create_user("Stream A", "pl").await.unwrap();
        let bartek = store.create_user("Stream B", "pl").await.unwrap();
        let turn_a = store.insert_turn(anna.id).await.unwrap();
        let turn_b = store.insert_turn(bartek.id).await.unwrap();
        let first = store
            .append_stream(
                anna.id,
                turn_a.id,
                "reply.delta",
                serde_json::json!({"t": "a1"}),
            )
            .await
            .unwrap();
        let second = store
            .append_stream(
                anna.id,
                turn_a.id,
                "reply.delta",
                serde_json::json!({"t": "a2"}),
            )
            .await
            .unwrap();
        let other = store
            .append_stream(
                bartek.id,
                turn_b.id,
                "reply.delta",
                serde_json::json!({"t": "b"}),
            )
            .await
            .unwrap();
        assert!(second > first);
        let anna_all = store.stream_after(anna.id, 0).await.unwrap();
        assert_eq!(anna_all.len(), 2);
        assert!(anna_all.iter().all(|event| event.user_id == anna.id));
        assert!(anna_all.iter().all(|event| event.id != other));
        let anna_rest = store.stream_after(anna.id, first).await.unwrap();
        assert_eq!(anna_rest.len(), 1);
        assert_eq!(anna_rest[0].id, second);
        let bartek_all = store.stream_after(bartek.id, 0).await.unwrap();
        assert_eq!(bartek_all.len(), 1);
        assert_eq!(bartek_all[0].id, other);
        assert_eq!(bartek_all[0].user_id, bartek.id);
    }

    #[tokio::test]
    async fn turns_finish_and_fail() {
        let store = schema_store().await;
        let user = store.create_user("Turn", "pl").await.unwrap();
        let turn = store.insert_turn(user.id).await.unwrap();
        assert_eq!(turn.status, TurnStatus::Running);
        store.set_turn_user_text(turn.id, "cześć").await.unwrap();
        store
            .set_checkpoint(turn.id, serde_json::json!({"round": 1}))
            .await
            .unwrap();
        let running = store.running_turns().await.unwrap();
        assert!(running.iter().any(|item| item.id == turn.id));
        store
            .finish_turn(turn.id, "dzień dobry", Some("audio/turn.wav"))
            .await
            .unwrap();
        let done = store.turn(user.id, turn.id).await.unwrap();
        assert_eq!(done.status, TurnStatus::Done);
        assert_eq!(done.user_text.as_deref(), Some("cześć"));
        assert_eq!(done.reply_text.as_deref(), Some("dzień dobry"));
        assert_eq!(done.audio_path.as_deref(), Some("audio/turn.wav"));
        assert_eq!(done.checkpoint.unwrap()["round"], 1);
        assert!(store
            .running_turns()
            .await
            .unwrap()
            .iter()
            .all(|item| item.id != turn.id));

        let failed_turn = store.insert_turn(user.id).await.unwrap();
        store.fail_turn(failed_turn.id, "model down").await.unwrap();
        let failed = store.turn(user.id, failed_turn.id).await.unwrap();
        assert_eq!(failed.status, TurnStatus::Failed);
        assert_eq!(failed.error.as_deref(), Some("model down"));
        let hidden = store.turn(user.id, turn.id).await.unwrap();
        assert_eq!(hidden.user_id, user.id);
        assert_eq!(
            store
                .turn(failed.user_id, Uuid::new_v4())
                .await
                .unwrap_err(),
            Error::NotFound
        );
    }

    #[tokio::test]
    async fn nearby_candidates_filter_and_map_embeddings() {
        let store = schema_store().await;
        let host = store.create_user("Near Host", "pl").await.unwrap();
        store
            .apply_profile(
                host.id,
                &ProfilePatch {
                    like_tag: Some("chess".to_string()),
                    age_band: Some("30s".to_string()),
                    sportiness: Some(1),
                    ..ProfilePatch::default()
                },
            )
            .await
            .unwrap();
        store
            .remember(
                host.id,
                NewMemory {
                    durability: Durability::LongTerm,
                    key: "health".to_string(),
                    value: "private note".to_string(),
                    quote: Some("secret diagnosis".to_string()),
                    confidence: None,
                },
            )
            .await
            .unwrap();
        let vector = pgvector::Vector::from(vec![0.25_f32; 1024]);
        sqlx::query(
            "UPDATE profiles SET embedding = $2, embedding_model = 'test' WHERE user_id = $1",
        )
        .bind(host.id)
        .bind(&vector)
        .execute(store.pool())
        .await
        .unwrap();

        let origin = rank::LatLng {
            lat: 50.0,
            lng: 20.0,
        };
        let near = store
            .create_event(host.id, new_event("near", 50.002, 20.0, None, soon(2)))
            .await
            .unwrap();
        let far = store
            .create_event(host.id, new_event("far", 51.0, 20.0, None, soon(6)))
            .await
            .unwrap();
        let cancelled = store
            .create_event(host.id, new_event("cancelled", 50.0, 20.0, None, soon(2)))
            .await
            .unwrap();
        sqlx::query("UPDATE events SET status = 'cancelled' WHERE id = $1")
            .bind(cancelled.id)
            .execute(store.pool())
            .await
            .unwrap();
        let expired = store
            .create_event(
                host.id,
                new_event("old", 50.0, 20.0, None, Utc::now() - Duration::hours(48)),
            )
            .await
            .unwrap();
        store.join_event(near.id, host.id).await.unwrap();

        let now = Utc::now();
        let found = store
            .nearby_candidates(origin, 500.0, None, now)
            .await
            .unwrap();
        assert!(found.iter().any(|event| event.id == near.id));
        assert!(found.iter().all(|event| event.id != far.id));
        assert!(found.iter().all(|event| event.id != cancelled.id));
        assert!(found.iter().all(|event| event.id != expired.id));
        let near_hit = found.iter().find(|event| event.id == near.id).unwrap();
        assert!(near_hit.embedding.is_none());
        assert_eq!(near_hit.signed_count, 1);
        assert_eq!(near_hit.attendees.len(), 1);
        assert!(!near_hit.attendees[0].complements);
        assert_eq!(near_hit.attendees[0].age_band.as_deref(), Some("30s"));
        assert_eq!(near_hit.attendees[0].sportiness, Some(1));
        assert_eq!(near_hit.attendees[0].tags, vec!["chess".to_string()]);
        let embedded = near_hit.attendees[0].embedding.as_ref().unwrap();
        assert_eq!(embedded.len(), 1024);
        assert!((embedded[0] - 0.25).abs() < 1e-6);

        let bounds = rank::BBox {
            west: 19.5,
            south: 50.5,
            east: 21.0,
            north: 51.5,
        };
        let boxed = store
            .nearby_candidates(origin, 500.0, Some(bounds), now)
            .await
            .unwrap();
        assert!(boxed.iter().any(|event| event.id == far.id));
        assert!(boxed.iter().all(|event| event.id != near.id));

        let viewer = store.create_user("Viewer", "pl").await.unwrap();
        let people = store.people(viewer.id, origin).await.unwrap();
        let person = people.iter().find(|person| person.id == host.id).unwrap();
        assert_eq!(person.first_name, "Near Host");
        assert!(person.mobility.is_none());
        assert!(!person.complements);
        assert_eq!(person.tags, vec!["chess".to_string()]);
        assert!(person.tags.iter().all(|tag| tag != "secret diagnosis"));
        assert_eq!(person.embedding.as_ref().unwrap().len(), 1024);
        assert!((person.latitude - 50.002).abs() < 1e-6);
        assert!((person.longitude - 20.0).abs() < 1e-6);
        assert!(people.iter().all(|person| person.id != viewer.id));
    }

    #[tokio::test]
    async fn chat_messages_page_and_search() {
        let store = schema_store().await;
        let anna = store.create_user("Chat A", "pl").await.unwrap();
        let bartek = store.create_user("Chat B", "pl").await.unwrap();
        for (role, body) in [
            (ChatRole::User, "one"),
            (ChatRole::Assistant, "reply-one"),
            (ChatRole::User, "two tennis"),
            (ChatRole::Assistant, "reply-two"),
            (ChatRole::User, "three"),
            (ChatRole::Assistant, "reply-three"),
        ] {
            store.insert_message(anna.id, role, body).await.unwrap();
        }
        store
            .insert_message(bartek.id, ChatRole::User, "tennis secret")
            .await
            .unwrap();

        let recent = store.recent_turns(anna.id, 2).await.unwrap();
        let bodies: Vec<_> = recent.iter().map(|message| message.body.as_str()).collect();
        assert_eq!(
            bodies,
            vec!["two tennis", "reply-two", "three", "reply-three"]
        );
        let page = store.messages_before(anna.id, None, 2).await.unwrap();
        let page_bodies: Vec<_> = page.iter().map(|message| message.body.as_str()).collect();
        assert_eq!(page_bodies, vec!["three", "reply-three"]);
        let older = store
            .messages_before(anna.id, Some(page[0].id), 2)
            .await
            .unwrap();
        let older_bodies: Vec<_> = older.iter().map(|message| message.body.as_str()).collect();
        assert_eq!(older_bodies, vec!["two tennis", "reply-two"]);
        let found = store.search_messages(anna.id, "tennis").await.unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].body, "two tennis");
        assert_eq!(found[0].user_id, anna.id);
        assert!(
            store
                .search_messages(bartek.id, "two tennis")
                .await
                .unwrap()
                .is_empty()
                || store
                    .search_messages(bartek.id, "two")
                    .await
                    .unwrap()
                    .iter()
                    .all(|message| message.user_id == bartek.id)
        );
        let bartek_hits = store.search_messages(bartek.id, "secret").await.unwrap();
        assert_eq!(bartek_hits.len(), 1);
        assert!(store
            .search_messages(anna.id, "secret")
            .await
            .unwrap()
            .is_empty());
        assert_eq!(store.count_user_messages(anna.id).await.unwrap(), 3);
    }

    #[tokio::test]
    async fn live_styrta_has_app_tables() {
        let url = live_url();
        assert_eq!(database_name(url), "styrta");
        let pool = quiet_pool(url).await;
        let before = ingest_counts(&pool).await;
        let store = Store::connect(url)
            .await
            .unwrap_or_else(|err| panic!("migrate styrta: {err}"));
        Store::connect(url)
            .await
            .unwrap_or_else(|err| panic!("second migrate styrta: {err}"));
        let after = ingest_counts(store.pool()).await;
        assert_eq!(before, after, "ingest tables changed");
        let tables: Vec<String> = sqlx::query_scalar(
            "SELECT tablename FROM pg_tables WHERE schemaname = 'public' AND tablename = ANY($1) ORDER BY tablename",
        )
        .bind(vec![
            "attendances",
            "conversations",
            "events",
            "memories",
            "messages",
            "places",
            "profile_tags",
            "profiles",
            "stream_events",
            "turns",
            "users",
        ])
        .fetch_all(store.pool())
        .await
        .unwrap();
        assert_eq!(
            tables,
            vec![
                "attendances",
                "conversations",
                "events",
                "memories",
                "messages",
                "places",
                "profile_tags",
                "profiles",
                "stream_events",
                "turns",
                "users",
            ]
        );
        let token_columns: i64 = sqlx::query_scalar(
            "SELECT count(*)::bigint FROM information_schema.columns WHERE table_schema = 'public' AND table_name = 'users' AND column_name = 'token'",
        )
        .fetch_one(store.pool())
        .await
        .unwrap();
        assert_eq!(token_columns, 0);
        let sportiness: i64 = sqlx::query_scalar(
            "SELECT count(*)::bigint FROM pg_constraint WHERE conname = 'profiles_sportiness_range'",
        )
        .fetch_one(store.pool())
        .await
        .unwrap();
        assert_eq!(sportiness, 1);
        let versions: Vec<i64> =
            sqlx::query_scalar("SELECT version FROM _sqlx_migrations ORDER BY version")
                .fetch_all(store.pool())
                .await
                .unwrap();
        assert_eq!(versions, vec![1, 2]);
    }

    async fn ingest_counts(pool: &PgPool) -> (i64, i64, i64, i64, i64) {
        sqlx::query_as(
            "SELECT
                (SELECT count(*)::bigint FROM innovations),
                (SELECT count(*)::bigint FROM innovation_links),
                (SELECT count(*)::bigint FROM innovation_files),
                (SELECT count(*)::bigint FROM innovation_chunks),
                (SELECT count(*)::bigint FROM ingest_runs)",
        )
        .fetch_one(pool)
        .await
        .unwrap_or_else(|err| panic!("counts: {}", redact(&err.to_string())))
    }
}
