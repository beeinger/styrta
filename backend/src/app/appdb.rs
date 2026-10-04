use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::embed::{Embedder, Input, DIMENSION};
use crate::rank;
pub use crate::rank::{EventStatus, PlaceKind, TimeWindow};

/// Mean Earth radius in meters. Distance uses haversine on float8 lat/lon.
const EARTH_RADIUS_M: f64 = 6_371_000.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    NotFound,
    EmptyTopic,
    Capacity,
    Taken,
    Invalid,
    Database(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => f.write_str("not found"),
            Self::EmptyTopic => f.write_str("empty topic"),
            Self::Capacity => f.write_str("event is full"),
            Self::Taken => f.write_str("email taken"),
            Self::Invalid => f.write_str("invalid value"),
            Self::Database(message) => write!(f, "database error: {message}"),
        }
    }
}

impl std::error::Error for Error {}

#[derive(Clone)]
pub struct Store {
    pool: PgPool,
    embedder: Option<Arc<dyn Embedder>>,
    embed_model: Arc<str>,
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
        Ok(Self {
            pool,
            embedder: None,
            embed_model: Arc::from(""),
        })
    }

    pub fn with_embedder(self, embedder: Arc<dyn Embedder>, model: &str) -> Self {
        Self {
            pool: self.pool,
            embedder: Some(embedder),
            embed_model: Arc::from(model),
        }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub async fn create_user(&self, new: &NewUser) -> Result<User, Error> {
        let email = crate::auth::normalize_email(&new.email).ok_or(Error::Invalid)?;
        let display_name = new.display_name.trim();
        let locale = new.locale.trim();
        if display_name.is_empty() || locale.is_empty() || new.password_hash.is_empty() {
            return Err(Error::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(db)?;
        let user: User = sqlx::query_as(
            r#"
            INSERT INTO users (email, password_hash, display_name, locale)
            VALUES ($1, $2, $3, $4)
            RETURNING id, email, display_name, locale
            "#,
        )
        .bind(&email)
        .bind(&new.password_hash)
        .bind(display_name)
        .bind(locale)
        .fetch_one(&mut *tx)
        .await
        .map_err(db)?;
        sqlx::query("INSERT INTO profiles (user_id) VALUES ($1)")
            .bind(user.id)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        sqlx::query("INSERT INTO conversations (user_id) VALUES ($1)")
            .bind(user.id)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        tx.commit().await.map_err(db)?;
        Ok(user)
    }

    pub async fn user_by_email(&self, email: &str) -> Result<Option<PasswordUser>, Error> {
        let Some(email) = crate::auth::normalize_email(email) else {
            return Ok(None);
        };
        let row: Option<CredentialRow> = sqlx::query_as(
            r#"
            SELECT id, email, display_name, locale, password_hash
            FROM users
            WHERE email = $1
            "#,
        )
        .bind(&email)
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        Ok(row.map(PasswordUser::from))
    }

    pub async fn user(&self, id: Uuid) -> Result<User, Error> {
        sqlx::query_as("SELECT id, email, display_name, locale FROM users WHERE id = $1")
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
        if patch.bio.is_some() || patch.like_tag.is_some() || patch.dislike_tag.is_some() {
            self.refresh_profile_embedding(user_id).await;
        }
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
        let saved = memory_from_row(row)?;
        if saved.durability == Durability::LongTerm {
            self.refresh_profile_embedding(user_id).await;
        }
        Ok(saved)
    }

    pub async fn forget(&self, user_id: Uuid, topic: &str) -> Result<ForgetResult, Error> {
        let topic = topic.trim();
        if topic.is_empty() {
            return Err(Error::EmptyTopic);
        }
        let pattern = like_contains(topic);
        let mut tx = self.pool.begin().await.map_err(db)?;
        let memories: Vec<MemoryRow> = sqlx::query_as(
            r#"
            WITH removed AS (
                DELETE FROM memories
                WHERE user_id = $1
                  AND (
                      key ILIKE $2 ESCAPE '\'
                      OR value ILIKE $2 ESCAPE '\'
                      OR coalesce(quote, '') ILIKE $2 ESCAPE '\'
                  )
                RETURNING id, durability, key, value, quote, confidence, confirmed
            )
            SELECT id, durability, key, value, quote, confidence, confirmed
            FROM removed
            ORDER BY key, id
            "#,
        )
        .bind(user_id)
        .bind(&pattern)
        .fetch_all(&mut *tx)
        .await
        .map_err(db)?;
        let tags: Vec<String> = sqlx::query_scalar(
            r#"
            WITH removed AS (
                DELETE FROM profile_tags
                WHERE user_id = $1 AND tag ILIKE $2 ESCAPE '\'
                RETURNING tag
            )
            SELECT tag FROM removed ORDER BY tag
            "#,
        )
        .bind(user_id)
        .bind(&pattern)
        .fetch_all(&mut *tx)
        .await
        .map_err(db)?;
        let refresh = memories
            .iter()
            .any(|memory| memory.durability == "long_term")
            || !tags.is_empty();
        tx.commit().await.map_err(db)?;
        if refresh {
            self.refresh_profile_embedding(user_id).await;
        }
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
        let created = self.event(event_id).await?;
        self.write_event_embedding(&created).await;
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
        viewer: Uuid,
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
        let embeddings: HashMap<Uuid, Vec<f32>> = rows
            .iter()
            .filter_map(|row| {
                row.embedding
                    .as_ref()
                    .map(|vector| (row.id, vector.to_vec()))
            })
            .collect();
        let events = rows
            .into_iter()
            .map(event_from_row)
            .collect::<Result<Vec<_>, _>>()?;
        let attendees = self.going_attendees(viewer, &events).await?;
        Ok(events
            .into_iter()
            .map(|event| {
                let embedding = embeddings.get(&event.id).cloned();
                let attendees = attendees.get(&event.id).cloned().unwrap_or_default();
                candidate(event, embedding, attendees)
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
                   COALESCE(
                       (SELECT jsonb_agg(jsonb_build_object('key', m.key, 'value', m.value) ORDER BY m.key, m.id)
                        FROM memories m
                        WHERE m.user_id = u.id AND m.durability = 'long_term'),
                       '[]'::jsonb
                   ) AS notes,
                   loc.lat,
                   loc.lon
            FROM users u
            JOIN profiles pr ON pr.user_id = u.id
            JOIN LATERAL (
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
        let viewer_profile = self.profile(viewer).await?;
        let viewer_notes = long_term_notes(&self.memories(viewer).await?);
        rows.into_iter()
            .map(|row| person_from_row(row, &viewer_profile.likes, &viewer_notes))
            .collect::<Result<Vec<_>, _>>()
            .map(|people| people.into_iter().flatten().collect())
    }

    pub async fn conversation(&self, user_id: Uuid) -> Result<Conversation, Error> {
        let row: ConversationRow = sqlx::query_as(
            "SELECT user_id, summary, summary_through, pending_place FROM conversations WHERE user_id = $1",
        )
        .bind(user_id)
        .fetch_one(&self.pool)
        .await
        .map_err(db)?;
        let pending_place = match row.pending_place {
            Some(value) => Some(serde_json::from_value(value).map_err(|_| Error::Invalid)?),
            None => None,
        };
        Ok(Conversation {
            user_id: row.user_id,
            summary: row.summary,
            summary_through: row.summary_through,
            pending_place,
        })
    }

    pub async fn set_pending_place(
        &self,
        user_id: Uuid,
        place: &PendingPlace,
    ) -> Result<(), Error> {
        let value = serde_json::to_value(place).map_err(|_| Error::Invalid)?;
        let updated = sqlx::query("UPDATE conversations SET pending_place = $2 WHERE user_id = $1")
            .bind(user_id)
            .bind(value)
            .execute(&self.pool)
            .await
            .map_err(db)?
            .rows_affected();
        if updated == 0 {
            Err(Error::NotFound)
        } else {
            Ok(())
        }
    }

    pub async fn clear_pending_place(&self, user_id: Uuid) -> Result<(), Error> {
        sqlx::query("UPDATE conversations SET pending_place = NULL WHERE user_id = $1")
            .bind(user_id)
            .execute(&self.pool)
            .await
            .map_err(db)?;
        Ok(())
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
        viewer: Uuid,
        events: &[Event],
    ) -> Result<HashMap<Uuid, Vec<rank::Attendee>>, Error> {
        let ids: Vec<Uuid> = events.iter().map(|event| event.id).collect();
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let viewer_profile = self.profile(viewer).await?;
        let viewer_notes = long_term_notes(&self.memories(viewer).await?);
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
                   ) AS tags,
                   COALESCE(
                       (SELECT jsonb_agg(jsonb_build_object('key', m.key, 'value', m.value) ORDER BY m.key, m.id)
                        FROM memories m
                        WHERE m.user_id = a.user_id AND m.durability = 'long_term'),
                       '[]'::jsonb
                   ) AS notes
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
            let notes = note_pairs(&row.notes);
            grouped
                .entry(row.event_id)
                .or_default()
                .push(rank::Attendee {
                    age_band: row.age_band,
                    sportiness: row.sportiness,
                    tags: row.tags.clone(),
                    embedding: row.embedding.map(|vector| vector.to_vec()),
                    complements: rank::company_complements(
                        &viewer_profile.likes,
                        &viewer_notes,
                        &row.tags,
                        &notes,
                    ),
                });
        }
        Ok(grouped)
    }

    async fn refresh_profile_embedding(&self, user_id: Uuid) {
        let Some(embedder) = &self.embedder else {
            return;
        };
        let model = self.embed_model.clone();
        let profile = match self.profile(user_id).await {
            Ok(profile) => profile,
            Err(err) => {
                tracing::warn!(%user_id, error = %err, "profile embedding skipped");
                return;
            }
        };
        let memories = match self.memories(user_id).await {
            Ok(memories) => memories,
            Err(err) => {
                tracing::warn!(%user_id, error = %err, "profile embedding skipped");
                return;
            }
        };
        let paragraph = profile_paragraph(&profile, &memories);
        if paragraph.is_empty() {
            if let Err(err) = self
                .store_profile_vector(user_id, None, Some(model.as_ref()))
                .await
            {
                tracing::warn!(%user_id, error = %err, "profile embedding clear failed");
            }
            return;
        }
        match embedder.embed(Input::Passage, &[paragraph]).await {
            Ok(vectors) => {
                let vector = vectors
                    .into_iter()
                    .next()
                    .filter(|vector| vector.len() == DIMENSION);
                if vector.is_none() {
                    tracing::warn!(%user_id, "profile embedding width or count was wrong");
                }
                if let Err(err) = self.store_profile_vector(user_id, vector, None).await {
                    tracing::warn!(%user_id, error = %err, "profile embedding write failed");
                }
            }
            Err(err) => {
                tracing::warn!(%user_id, error = %err, "profile embedding failed");
                if let Err(err) = self.store_profile_vector(user_id, None, None).await {
                    tracing::warn!(%user_id, error = %err, "profile embedding clear failed");
                }
            }
        }
    }

    async fn store_profile_vector(
        &self,
        user_id: Uuid,
        vector: Option<Vec<f32>>,
        model: Option<&str>,
    ) -> Result<(), Error> {
        let model = vector.as_ref().map(|_| self.embed_model.as_ref()).or(model);
        sqlx::query("UPDATE profiles SET embedding = $2, embedding_model = $3 WHERE user_id = $1")
            .bind(user_id)
            .bind(vector.map(pgvector::Vector::from))
            .bind(model)
            .execute(&self.pool)
            .await
            .map_err(db)?;
        Ok(())
    }

    async fn write_event_embedding(&self, event: &Event) {
        let Some(embedder) = &self.embedder else {
            return;
        };
        let passage = event_passage(event);
        match embedder.embed(Input::Passage, &[passage]).await {
            Ok(vectors) => {
                let vector = vectors
                    .into_iter()
                    .next()
                    .filter(|vector| vector.len() == DIMENSION);
                let model = vector.as_ref().map(|_| self.embed_model.as_ref());
                if vector.is_none() {
                    tracing::warn!(event_id = %event.id, "event embedding width or count was wrong");
                }
                if let Err(err) = self.store_event_vector(event.id, vector, model).await {
                    tracing::warn!(event_id = %event.id, error = %err, "event embedding write failed");
                }
            }
            Err(err) => {
                tracing::warn!(event_id = %event.id, error = %err, "event embedding failed");
                if let Err(err) = self.store_event_vector(event.id, None, None).await {
                    tracing::warn!(event_id = %event.id, error = %err, "event embedding clear failed");
                }
            }
        }
    }

    async fn store_event_vector(
        &self,
        event_id: Uuid,
        vector: Option<Vec<f32>>,
        model: Option<&str>,
    ) -> Result<(), Error> {
        sqlx::query("UPDATE events SET embedding = $2, embedding_model = $3 WHERE id = $1")
            .bind(event_id)
            .bind(vector.map(pgvector::Vector::from))
            .bind(model)
            .execute(&self.pool)
            .await
            .map_err(db)?;
        Ok(())
    }

    pub async fn fill_embeddings(&self) -> Result<(u64, u64, u64), Error> {
        let embedder = self.embedder.as_ref().ok_or(Error::Invalid)?;
        let model = self.embed_model.as_ref();
        let profiles: Vec<Uuid> = sqlx::query_scalar(
            "SELECT user_id FROM profiles WHERE embedding_model IS DISTINCT FROM $1",
        )
        .bind(model)
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        let profile_count = profiles.len() as u64;
        for user_id in profiles {
            self.refresh_profile_embedding(user_id).await;
        }
        let events: Vec<Uuid> =
            sqlx::query_scalar("SELECT id FROM events WHERE embedding_model IS DISTINCT FROM $1")
                .bind(model)
                .fetch_all(&self.pool)
                .await
                .map_err(db)?;
        let event_count = events.len() as u64;
        for event_id in events {
            if let Ok(event) = self.event(event_id).await {
                self.write_event_embedding(&event).await;
            }
        }
        let chunks = crate::knowledge::embed_missing(&self.pool, embedder.as_ref(), model)
            .await
            .map_err(|err| Error::Database(err.to_string()))?;
        Ok((profile_count, event_count, chunks))
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
    e.embedding,
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
        sqlx::Error::Database(inner)
            if inner.code().as_deref() == Some("23505")
                && inner.constraint() == Some("users_email_key") =>
        {
            Error::Taken
        }
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

fn candidate(
    event: Event,
    embedding: Option<Vec<f32>>,
    attendees: Vec<rank::Attendee>,
) -> rank::Candidate {
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
        embedding,
        attendees,
    }
}

fn person_from_row(
    row: PersonRow,
    viewer_likes: &[String],
    viewer_notes: &[(String, String)],
) -> Result<Option<rank::Person>, Error> {
    let (Some(latitude), Some(longitude)) = (row.lat, row.lon) else {
        return Ok(None);
    };
    let notes = note_pairs(&row.notes);
    Ok(Some(rank::Person {
        id: row.id,
        first_name: row.first_name,
        age_band: row.age_band,
        gender: row.gender,
        mobility: None,
        sportiness: row.sportiness,
        tags: row.tags.clone(),
        women_only: row.women_only,
        time_window: time_window(row.start_minute, row.end_minute)?,
        embedding: row.embedding.map(|vector| vector.to_vec()),
        latitude,
        longitude,
        complements: rank::company_complements(viewer_likes, viewer_notes, &row.tags, &notes),
    }))
}

pub fn profile_paragraph(profile: &Profile, memories: &[Memory]) -> String {
    let mut lines = Vec::new();
    if let Some(bio) = profile
        .bio
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        lines.push(bio.to_string());
    }
    if !profile.likes.is_empty() {
        lines.push(format!("likes: {}", profile.likes.join(", ")));
    }
    if !profile.dislikes.is_empty() {
        lines.push(format!("dislikes: {}", profile.dislikes.join(", ")));
    }
    let mut notes: Vec<&Memory> = memories
        .iter()
        .filter(|memory| memory.durability == Durability::LongTerm)
        .collect();
    notes.sort_by(|left, right| left.key.cmp(&right.key).then(left.id.cmp(&right.id)));
    for memory in notes {
        let key = memory.key.trim();
        let value = memory.value.trim();
        if key.is_empty() && value.is_empty() {
            continue;
        }
        lines.push(format!("{key}: {value}"));
    }
    lines.join("\n")
}

fn event_passage(event: &Event) -> String {
    let mut lines = vec![event.title.trim().to_string()];
    if let Some(description) = event
        .description
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        lines.push(description.to_string());
    }
    if !event.activity_tags.is_empty() {
        lines.push(event.activity_tags.join(", "));
    }
    lines.join("\n")
}

fn long_term_notes(memories: &[Memory]) -> Vec<(String, String)> {
    memories
        .iter()
        .filter(|memory| memory.durability == Durability::LongTerm)
        .map(|memory| (memory.key.clone(), memory.value.clone()))
        .collect()
}

fn note_pairs(value: &Value) -> Vec<(String, String)> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    Some((
                        item.get("key")?.as_str()?.to_string(),
                        item.get("value")?.as_str()?.to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
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
    embedding: Option<pgvector::Vector>,
    signed_count: i64,
}

#[derive(sqlx::FromRow)]
struct AttendeeRow {
    event_id: Uuid,
    age_band: Option<String>,
    sportiness: Option<i16>,
    embedding: Option<pgvector::Vector>,
    tags: Vec<String>,
    notes: Value,
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
    notes: Value,
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
    pub email: Option<String>,
    pub display_name: String,
    pub locale: String,
}

pub struct NewUser {
    pub email: String,
    pub password_hash: String,
    pub display_name: String,
    pub locale: String,
}

pub struct PasswordUser {
    pub user: User,
    pub password_hash: String,
}

#[derive(sqlx::FromRow)]
struct CredentialRow {
    id: Uuid,
    email: String,
    display_name: String,
    locale: String,
    password_hash: String,
}

impl From<CredentialRow> for PasswordUser {
    fn from(row: CredentialRow) -> Self {
        Self {
            user: User {
                id: row.id,
                email: Some(row.email),
                display_name: row.display_name,
                locale: row.locale,
            },
            password_hash: row.password_hash,
        }
    }
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

/// Partial profile update. Only fields that are present are written.
#[derive(Clone, Debug, Default, Serialize, Deserialize, ToSchema)]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
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

#[derive(Clone, Debug, PartialEq)]
pub struct Conversation {
    pub user_id: Uuid,
    pub summary: Option<String>,
    pub summary_through: Option<Uuid>,
    pub pending_place: Option<PendingPlace>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PendingPlace {
    pub name: String,
    pub street: String,
    pub city: String,
    pub address: String,
    pub lat: f64,
    pub lon: f64,
    pub kind: PlaceKind,
}

#[derive(sqlx::FromRow)]
struct ConversationRow {
    user_id: Uuid,
    summary: Option<String>,
    summary_through: Option<Uuid>,
    pending_place: Option<Value>,
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
    use std::sync::Mutex;

    #[derive(Default)]
    struct RecordingEmbedder {
        calls: Mutex<Vec<(Input, Vec<String>)>>,
    }

    #[async_trait::async_trait]
    impl Embedder for RecordingEmbedder {
        async fn embed(
            &self,
            input: Input,
            texts: &[String],
        ) -> Result<Vec<Vec<f32>>, crate::embed::Error> {
            self.calls
                .lock()
                .expect("calls")
                .push((input, texts.to_vec()));
            Ok(texts.iter().map(|_| vec![0.2; DIMENSION]).collect())
        }
    }

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
                assert_eq!(versions, vec![1, 2, 3, 4, 5]);
                let pending: i64 = sqlx::query_scalar(
                    "SELECT count(*)::bigint FROM information_schema.columns WHERE table_name = 'conversations' AND column_name = 'pending_place'",
                )
                .fetch_one(store.pool())
                .await
                .unwrap_or_else(|err| panic!("pending column: {}", redact(&err.to_string())));
                assert_eq!(pending, 1);
                let credentials: i64 = sqlx::query_scalar(
                    "SELECT count(*)::bigint FROM information_schema.columns WHERE table_name = 'users' AND column_name IN ('email', 'password_hash')",
                )
                .fetch_one(store.pool())
                .await
                .unwrap_or_else(|err| panic!("credential columns: {}", redact(&err.to_string())));
                assert_eq!(credentials, 2);
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

    async fn add_user(store: &Store, name: &str) -> User {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let slug: String = name
            .chars()
            .filter(|ch| ch.is_ascii_alphanumeric())
            .flat_map(|ch| ch.to_lowercase())
            .collect();
        let slug = if slug.is_empty() {
            "user".to_string()
        } else {
            slug
        };
        store
            .create_user(&NewUser {
                email: format!("{slug}-{n}@example.test"),
                password_hash: "fixture".into(),
                display_name: name.into(),
                locale: "pl".into(),
            })
            .await
            .unwrap_or_else(|err| panic!("{err}"))
    }

    #[tokio::test]
    async fn create_user_opens_profile_and_conversation() {
        let store = schema_store().await;
        let user = add_user(&store, "Anna").await;
        assert_eq!(user.display_name, "Anna");
        assert!(user.email.as_deref().unwrap().ends_with("@example.test"));
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
        assert!(conversation.pending_place.is_none());
    }

    #[tokio::test]
    async fn email_is_unique_and_legacy_rows_have_none() {
        let store = schema_store().await;
        let created = store
            .create_user(&NewUser {
                email: " Ada@Example.TEST ".into(),
                password_hash: "fixture".into(),
                display_name: "Ada".into(),
                locale: "pl".into(),
            })
            .await
            .unwrap();
        assert_eq!(created.email.as_deref(), Some("ada@example.test"));
        let again = store
            .create_user(&NewUser {
                email: "ada@example.test".into(),
                password_hash: "other".into(),
                display_name: "Ada".into(),
                locale: "pl".into(),
            })
            .await
            .unwrap_err();
        assert_eq!(again, Error::Taken);
        let found = store
            .user_by_email("ADA@example.test")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.user.id, created.id);
        assert_eq!(found.password_hash, "fixture");
        assert!(store
            .user_by_email("missing@example.test")
            .await
            .unwrap()
            .is_none());
        assert!(store.user_by_email("not-an-email").await.unwrap().is_none());

        sqlx::query("INSERT INTO users (display_name) VALUES ('Legacy')")
            .execute(store.pool())
            .await
            .unwrap();
        let err = sqlx::query(
            "INSERT INTO users (display_name, email) VALUES ('Half', 'half@example.test')",
        )
        .execute(store.pool())
        .await
        .unwrap_err();
        assert!(
            err.to_string().contains("users_credentials_pair") || err.to_string().contains("23514")
        );
    }

    #[tokio::test]
    async fn pending_place_round_trips_and_clears() {
        let store = schema_store().await;
        let user = add_user(&store, "Arena").await;
        let place = PendingPlace {
            name: "Tauron Arena Kraków".into(),
            street: "Stanisława Lema 7".into(),
            city: "Kraków".into(),
            address: "Stanisława Lema 7, Kraków".into(),
            lat: 50.0677202,
            lon: 19.9915490,
            kind: PlaceKind::Hall,
        };
        store.set_pending_place(user.id, &place).await.unwrap();
        let loaded = store.conversation(user.id).await.unwrap();
        assert_eq!(
            loaded.pending_place.as_ref().unwrap().name,
            "Tauron Arena Kraków"
        );
        assert_eq!(loaded.pending_place.as_ref().unwrap().kind, PlaceKind::Hall);
        assert!((loaded.pending_place.as_ref().unwrap().lat - 50.0677202).abs() < 1e-9);
        store.clear_pending_place(user.id).await.unwrap();
        assert!(store
            .conversation(user.id)
            .await
            .unwrap()
            .pending_place
            .is_none());
    }

    #[tokio::test]
    async fn forget_topic_removes_tennis_and_keeps_coffee() {
        let store = schema_store().await;
        let user = add_user(&store, "Basia").await;
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
        let host = add_user(&store, "Host").await;
        let guest = add_user(&store, "Guest").await;
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
        let anna = add_user(&store, "Stream A").await;
        let bartek = add_user(&store, "Stream B").await;
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
        let user = add_user(&store, "Turn").await;
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
        let host = add_user(&store, "Near Host").await;
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
            .nearby_candidates(host.id, origin, 500.0, None, now)
            .await
            .unwrap();
        assert!(found.iter().any(|event| event.id == near.id));
        assert!(found.iter().all(|event| event.id != far.id));
        assert!(found.iter().all(|event| event.id != cancelled.id));
        assert!(found.iter().all(|event| event.id != expired.id));
        let near_hit = found.iter().find(|event| event.id == near.id).unwrap();
        assert!(near_hit.embedding.is_none());
        let event_vector = pgvector::Vector::from(vec![0.5_f32; 1024]);
        sqlx::query("UPDATE events SET embedding = $2 WHERE id = $1")
            .bind(near.id)
            .bind(&event_vector)
            .execute(store.pool())
            .await
            .unwrap();
        let found = store
            .nearby_candidates(host.id, origin, 500.0, None, now)
            .await
            .unwrap();
        let near_hit = found.iter().find(|event| event.id == near.id).unwrap();
        assert_eq!(near_hit.embedding.as_ref().unwrap().len(), 1024);
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
            .nearby_candidates(host.id, origin, 500.0, Some(bounds), now)
            .await
            .unwrap();
        assert!(boxed.iter().any(|event| event.id == far.id));
        assert!(boxed.iter().all(|event| event.id != near.id));

        let viewer = add_user(&store, "Viewer").await;
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
        let nowhere = add_user(&store, "Nowhere").await;
        let people = store.people(viewer.id, origin).await.unwrap();
        assert!(people.iter().all(|person| person.id != nowhere.id));
    }

    #[test]
    fn profile_paragraph_omits_quotes_mobility_and_short_term() {
        let profile = Profile {
            user_id: Uuid::nil(),
            age_band: None,
            gender: None,
            mobility: Some("wheelchair".into()),
            sportiness: None,
            bio: Some("likes parks".into()),
            likes: vec!["chess".into()],
            dislikes: vec!["golf".into()],
            women_only: false,
            time_window: None,
            embedding: None,
            embedding_model: None,
        };
        let memories = vec![
            Memory {
                id: Uuid::from_u128(1),
                durability: Durability::LongTerm,
                key: "home".into(),
                value: "lives alone".into(),
                quote: Some("secret quote".into()),
                confidence: None,
                confirmed: true,
            },
            Memory {
                id: Uuid::from_u128(2),
                durability: Durability::ShortTerm,
                key: "today".into(),
                value: "ephemeral errand".into(),
                quote: None,
                confidence: None,
                confirmed: true,
            },
        ];
        let text = profile_paragraph(&profile, &memories);
        assert!(text.contains("likes parks"), "{text}");
        assert!(text.contains("likes: chess"), "{text}");
        assert!(text.contains("dislikes: golf"), "{text}");
        assert!(text.contains("home: lives alone"), "{text}");
        assert!(!text.contains("wheelchair"), "{text}");
        assert!(!text.contains("secret quote"), "{text}");
        assert!(!text.contains("ephemeral"), "{text}");
        assert!(profile_paragraph(
            &Profile {
                bio: None,
                likes: vec![],
                dislikes: vec![],
                ..profile
            },
            &[],
        )
        .is_empty());
    }

    #[tokio::test]
    async fn long_term_memory_reembeds_and_short_term_does_not() {
        let recorder = Arc::new(RecordingEmbedder::default());
        let store = schema_store()
            .await
            .with_embedder(Arc::clone(&recorder) as Arc<dyn Embedder>, "test-model");
        let user = add_user(&store, "Embed").await;
        store
            .remember(
                user.id,
                NewMemory {
                    durability: Durability::ShortTerm,
                    key: "today".into(),
                    value: "ephemeral errand".into(),
                    quote: None,
                    confidence: None,
                },
            )
            .await
            .unwrap();
        let profile = store.profile(user.id).await.unwrap();
        assert!(profile.embedding.is_none());
        assert!(recorder.calls.lock().unwrap().is_empty());

        store
            .remember(
                user.id,
                NewMemory {
                    durability: Durability::LongTerm,
                    key: "home".into(),
                    value: "lives alone".into(),
                    quote: Some("secret quote".into()),
                    confidence: None,
                },
            )
            .await
            .unwrap();
        let profile = store.profile(user.id).await.unwrap();
        assert!(profile.embedding.is_some());
        assert_eq!(profile.embedding_model.as_deref(), Some("test-model"));
        let calls = recorder.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, Input::Passage);
        assert!(
            calls[0].1[0].contains("home: lives alone"),
            "{}",
            calls[0].1[0]
        );
        assert!(!calls[0].1[0].contains("secret quote"), "{}", calls[0].1[0]);
        assert!(!calls[0].1[0].contains("ephemeral"), "{}", calls[0].1[0]);
    }

    #[tokio::test]
    async fn company_complement_is_set_on_attendees_and_people() {
        let store = schema_store().await;
        let lonely = add_user(&store, "Lonely").await;
        let company = add_user(&store, "Company").await;
        let other = add_user(&store, "Other").await;
        for (id, tag) in [
            (lonely.id, "samotna"),
            (company.id, "towarzystwo"),
            (other.id, "coffee"),
        ] {
            store
                .apply_profile(
                    id,
                    &ProfilePatch {
                        like_tag: Some(tag.into()),
                        ..ProfilePatch::default()
                    },
                )
                .await
                .unwrap();
        }
        let event = store
            .create_event(lonely.id, new_event("walk", 50.0, 20.0, None, soon(2)))
            .await
            .unwrap();
        store.join_event(event.id, lonely.id).await.unwrap();
        store.join_event(event.id, company.id).await.unwrap();
        store.join_event(event.id, other.id).await.unwrap();
        let origin = rank::LatLng {
            lat: 50.0,
            lng: 20.0,
        };
        let found = store
            .nearby_candidates(lonely.id, origin, 5_000.0, None, Utc::now())
            .await
            .unwrap();
        let hit = found.iter().find(|item| item.id == event.id).unwrap();
        let company_row = hit
            .attendees
            .iter()
            .find(|attendee| attendee.tags.iter().any(|tag| tag == "towarzystwo"))
            .unwrap();
        let other_row = hit
            .attendees
            .iter()
            .find(|attendee| attendee.tags.iter().any(|tag| tag == "coffee"))
            .unwrap();
        assert!(company_row.complements);
        assert!(!other_row.complements);
        let people = store.people(lonely.id, origin).await.unwrap();
        assert!(
            people
                .iter()
                .find(|person| person.id == company.id)
                .unwrap()
                .complements
        );
        assert!(
            !people
                .iter()
                .find(|person| person.id == other.id)
                .unwrap()
                .complements
        );
    }

    #[tokio::test]
    async fn chat_messages_page_and_search() {
        let store = schema_store().await;
        let anna = add_user(&store, "Chat A").await;
        let bartek = add_user(&store, "Chat B").await;
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
        assert_eq!(versions, vec![1, 2, 3, 4, 5]);
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
