use std::collections::HashSet;

use chrono::{DateTime, Datelike, Duration, Timelike, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

pub const PEOPLE_WEIGHT: f64 = 0.45;
pub const DISTANCE_WEIGHT: f64 = 0.25;
pub const TEXT_WEIGHT: f64 = 0.20;
pub const TIME_WEIGHT: f64 = 0.10;
pub const PROMOTED_BONUS: f64 = 0.05;
pub const DISTANCE_SCALE_M: f64 = 2500.0;
pub const MIN_RADIUS_M: f64 = 400.0;
pub const MAX_RADIUS_M: f64 = 30_000.0;
pub const VIEWPORT_METERS_PER_PIXEL: f64 = 156_543.03;
pub const VIEWPORT_PIXELS: f64 = 600.0;
pub const COLD_CAP: usize = 40;
pub const NEARBY_CAP: usize = 50;

const EARTH_RADIUS_M: f64 = 6_371_000.0;
const DAY_MINUTES: u16 = 24 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    NonFinite,
    BadBBox,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NonFinite => f.write_str("non-finite latitude or zoom"),
            Self::BadBBox => f.write_str("bbox must be four finite numbers: west,south,east,north"),
        }
    }
}

impl std::error::Error for Error {}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LatLng {
    pub lat: f64,
    pub lng: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TimeWindow {
    /// Inclusive start, minutes from local midnight.
    pub start_minute: u16,
    /// Exclusive end, minutes from local midnight.
    pub end_minute: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BBox {
    pub west: f64,
    pub south: f64,
    pub east: f64,
    pub north: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlaceKind {
    Cafe,
    Park,
    Hall,
    Square,
    OtherPublic,
    NotPublic,
}

impl PlaceKind {
    pub fn is_public(self) -> bool {
        !matches!(self, Self::NotPublic)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cafe => "cafe",
            Self::Park => "park",
            Self::Hall => "hall",
            Self::Square => "square",
            Self::OtherPublic => "other_public",
            Self::NotPublic => "not_public",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum EventStatus {
    Scheduled,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    pub age_band: Option<String>,
    pub gender: Option<String>,
    pub mobility: Option<String>,
    pub sportiness: Option<i16>,
    pub likes: Vec<String>,
    pub dislikes: Vec<String>,
    pub women_only: bool,
    pub time_window: Option<TimeWindow>,
    pub embedding: Option<Vec<f32>>,
}

impl Profile {
    pub fn is_empty(&self) -> bool {
        self.age_band.is_none()
            && self.gender.is_none()
            && self.mobility.is_none()
            && self.sportiness.is_none()
            && self.likes.is_empty()
            && self.dislikes.is_empty()
            && !self.women_only
            && self.time_window.is_none()
            && self.embedding.is_none()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Attendee {
    pub age_band: Option<String>,
    pub sportiness: Option<i16>,
    pub tags: Vec<String>,
    pub embedding: Option<Vec<f32>>,
    pub complements: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub id: Uuid,
    pub title: String,
    pub emoji: String,
    pub description: String,
    pub activity_tags: Vec<String>,
    pub women_only: bool,
    pub starts_at: DateTime<Utc>,
    pub capacity: Option<i32>,
    pub promoted: bool,
    pub status: EventStatus,
    pub place_name: String,
    pub place_kind: PlaceKind,
    pub latitude: f64,
    pub longitude: f64,
    pub host_name: String,
    pub signed_count: i64,
    pub embedding: Option<Vec<f32>>,
    pub attendees: Vec<Attendee>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScoredEvent {
    pub id: Uuid,
    pub score: f64,
    pub promoted: bool,
    pub distance_m: f64,
    pub title: String,
    pub emoji: String,
    pub signed_count: i64,
    pub capacity: Option<i32>,
    pub place_name: String,
    pub latitude: f64,
    pub longitude: f64,
    pub starts_at: DateTime<Utc>,
    pub host_name: String,
}

#[derive(Clone, Debug)]
pub struct RankInput<'a> {
    pub profile: &'a Profile,
    pub origin: LatLng,
    /// Used when `bounds` is absent.
    pub radius_m: f64,
    /// When set, selects events instead of `radius_m`.
    pub bounds: Option<BBox>,
    pub now: DateTime<Utc>,
    pub want_embedding: Option<&'a [f32]>,
    pub events: &'a [Candidate],
}

#[derive(Clone, Debug, PartialEq)]
pub struct Person {
    pub id: Uuid,
    pub first_name: String,
    pub age_band: Option<String>,
    pub gender: Option<String>,
    pub mobility: Option<String>,
    pub sportiness: Option<i16>,
    pub tags: Vec<String>,
    pub women_only: bool,
    pub time_window: Option<TimeWindow>,
    pub embedding: Option<Vec<f32>>,
    pub latitude: f64,
    pub longitude: f64,
    pub complements: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DistanceBand {
    Within500M,
    Within2Km,
    Within5Km,
    Farther,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PersonHit {
    pub first_name: String,
    pub age_band: Option<String>,
    pub shared_tags: Vec<String>,
    pub distance_band: DistanceBand,
    pub constraints_passed: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct PeopleInput<'a> {
    pub profile: &'a Profile,
    pub origin: LatLng,
    pub now: DateTime<Utc>,
    pub people: &'a [Person],
}

pub fn rank_events(input: &RankInput<'_>) -> Result<Vec<ScoredEvent>, Error> {
    let cold = input.profile.is_empty();
    let mut hits = Vec::new();
    for event in input.events {
        let Some(distance_m) = accept_location(
            input.origin,
            input.radius_m,
            input.bounds,
            event.latitude,
            event.longitude,
        ) else {
            continue;
        };
        if event_fact_drop(event, input.now) || preference_drop(input.profile, event) {
            continue;
        }
        let score = if cold {
            0.0
        } else {
            score_event(input.profile, event, distance_m, input.want_embedding)
        };
        hits.push(to_scored(event, score, distance_m));
    }
    if cold {
        hits.sort_by(|a, b| {
            a.starts_at
                .cmp(&b.starts_at)
                .then(a.distance_m.total_cmp(&b.distance_m))
                .then(a.id.cmp(&b.id))
        });
        hits.truncate(COLD_CAP);
    } else {
        hits.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then(a.distance_m.total_cmp(&b.distance_m))
                .then(a.starts_at.cmp(&b.starts_at))
                .then(a.id.cmp(&b.id))
        });
        hits.truncate(NEARBY_CAP);
    }
    Ok(hits)
}

pub fn rank_people(input: &PeopleInput<'_>) -> Result<Vec<PersonHit>, Error> {
    // People have a window, not a start time, so `now` is not a hard drop.
    let _ = input.now;
    let mut ranked = Vec::new();
    for person in input.people {
        if person_preference_drop(input.profile, person) {
            continue;
        }
        let Some(distance_m) = geo_distance(input.origin, person.latitude, person.longitude) else {
            continue;
        };
        let profile_cosine = match (
            input.profile.embedding.as_deref(),
            person.embedding.as_deref(),
        ) {
            (Some(profile), Some(person)) => Some(cosine(profile, person)),
            _ => None,
        };
        let similarity = similarity(
            input.profile,
            person.age_band.as_deref(),
            person.sportiness,
            &person.tags,
            person.embedding.as_deref(),
            person.complements,
        );
        ranked.push(RankedPerson {
            profile_cosine,
            similarity,
            distance_m,
            id: person.id,
            hit: PersonHit {
                first_name: person.first_name.clone(),
                age_band: person.age_band.clone(),
                shared_tags: shared_tags(&input.profile.likes, &person.tags),
                distance_band: distance_band(distance_m),
                constraints_passed: constraints_passed(input.profile, person),
            },
        });
    }
    ranked.sort_by(|a, b| {
        cmp_optional_desc(a.profile_cosine, b.profile_cosine)
            .then(b.similarity.total_cmp(&a.similarity))
            .then(a.distance_m.total_cmp(&b.distance_m))
            .then(a.id.cmp(&b.id))
    });
    Ok(ranked.into_iter().map(|row| row.hit).collect())
}

/// Map zoom to a search radius. `lat_deg` is degrees; cosine uses radians.
pub fn viewport_radius_m(lat_deg: f64, zoom: f64) -> Result<f64, Error> {
    if !lat_deg.is_finite() || !zoom.is_finite() {
        return Err(Error::NonFinite);
    }
    let raw =
        VIEWPORT_METERS_PER_PIXEL * lat_deg.to_radians().cos() / 2f64.powf(zoom) * VIEWPORT_PIXELS;
    Ok(raw.clamp(MIN_RADIUS_M, MAX_RADIUS_M))
}

pub fn parse_bbox(text: &str) -> Result<BBox, Error> {
    let parts: Vec<&str> = text.split(',').collect();
    if parts.len() != 4 {
        return Err(Error::BadBBox);
    }
    let mut nums = [0.0; 4];
    for (i, part) in parts.iter().enumerate() {
        let n: f64 = part.trim().parse().map_err(|_| Error::BadBBox)?;
        if !n.is_finite() {
            return Err(Error::BadBBox);
        }
        nums[i] = n;
    }
    Ok(BBox {
        west: nums[0],
        south: nums[1],
        east: nums[2],
        north: nums[3],
    })
}

struct RankedPerson {
    profile_cosine: Option<f64>,
    similarity: f64,
    distance_m: f64,
    id: Uuid,
    hit: PersonHit,
}

fn to_scored(event: &Candidate, score: f64, distance_m: f64) -> ScoredEvent {
    ScoredEvent {
        id: event.id,
        score,
        promoted: event.promoted,
        distance_m,
        title: event.title.clone(),
        emoji: event.emoji.clone(),
        signed_count: event.signed_count,
        capacity: event.capacity,
        place_name: event.place_name.clone(),
        latitude: event.latitude,
        longitude: event.longitude,
        starts_at: event.starts_at,
        host_name: event.host_name.clone(),
    }
}

fn score_event(profile: &Profile, event: &Candidate, distance_m: f64, want: Option<&[f32]>) -> f64 {
    let mut weighted = 0.0;
    let mut weight = 0.0;
    if let Some(people) = mean_attendee_similarity(profile, &event.attendees) {
        weighted += PEOPLE_WEIGHT * people;
        weight += PEOPLE_WEIGHT;
    }
    let near = (-distance_m / DISTANCE_SCALE_M).exp();
    weighted += DISTANCE_WEIGHT * near;
    weight += DISTANCE_WEIGHT;
    if let (Some(event_vec), Some(want_vec)) = (event.embedding.as_deref(), want) {
        weighted += TEXT_WEIGHT * cosine(event_vec, want_vec);
        weight += TEXT_WEIGHT;
    }
    let time = time_fit(profile.time_window, event.starts_at);
    weighted += TIME_WEIGHT * time;
    weight += TIME_WEIGHT;
    let base = weighted / weight;
    if event.promoted {
        base + PROMOTED_BONUS
    } else {
        base
    }
}

/// A missing cosine sorts after every real one. `sort_by` wants the higher value first.
fn cmp_optional_desc(a: Option<f64>, b: Option<f64>) -> std::cmp::Ordering {
    match (a, b) {
        (Some(a), Some(b)) => b.total_cmp(&a),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

/// Company fit is a rule, not a cosine. "Lonely" and "wants company" are far apart in embedding space.
pub fn company_complements(
    left_likes: &[String],
    left_notes: &[(String, String)],
    right_likes: &[String],
    right_notes: &[(String, String)],
) -> bool {
    let (left_lonely, left_company) = company_signals(left_likes, left_notes);
    let (right_lonely, right_company) = company_signals(right_likes, right_notes);
    if (left_lonely && right_company) || (right_lonely && left_company) {
        return true;
    }
    left_company && right_company && shares_like(left_likes, right_likes)
}

fn company_signals(likes: &[String], notes: &[(String, String)]) -> (bool, bool) {
    let mut lonely = false;
    let mut company = false;
    for text in likes.iter().map(String::as_str).chain(
        notes
            .iter()
            .flat_map(|(key, value)| [key.as_str(), value.as_str()]),
    ) {
        let folded = text.to_lowercase();
        if folded.contains("lonely") || folded.contains("lives alone") || folded.contains("samotn")
        {
            lonely = true;
        }
        if folded.contains("wants company") || folded.contains("towarzystwo") {
            company = true;
        }
    }
    (lonely, company)
}

fn shares_like(left: &[String], right: &[String]) -> bool {
    let right = tag_set(right);
    tag_set(left).iter().any(|tag| right.contains(tag))
}

fn mean_attendee_similarity(profile: &Profile, attendees: &[Attendee]) -> Option<f64> {
    if attendees.is_empty() {
        return None;
    }
    let sum: f64 = attendees
        .iter()
        .map(|attendee| {
            similarity(
                profile,
                attendee.age_band.as_deref(),
                attendee.sportiness,
                &attendee.tags,
                attendee.embedding.as_deref(),
                attendee.complements,
            )
        })
        .sum();
    Some(sum / attendees.len() as f64)
}

/// Equal-weight mean of the parts that exist. A missing embedding is omitted,
/// not stored as zero, when age, sportiness, tags, or a complement still exist.
fn similarity(
    profile: &Profile,
    age_band: Option<&str>,
    sportiness: Option<i16>,
    tags: &[String],
    embedding: Option<&[f32]>,
    complements: bool,
) -> f64 {
    let mut sum = 0.0;
    let mut n = 0u32;
    if let (Some(a), Some(b)) = (profile.age_band.as_deref(), age_band) {
        sum += age_similarity(a, b);
        n += 1;
    }
    if let (Some(a), Some(b)) = (profile.sportiness, sportiness) {
        sum += sportiness_similarity(a, b);
        n += 1;
    }
    if let Some(score) = jaccard(&profile.likes, tags) {
        sum += score;
        n += 1;
    }
    if let (Some(a), Some(b)) = (profile.embedding.as_deref(), embedding) {
        sum += cosine(a, b);
        n += 1;
    }
    if complements {
        sum += 1.0;
        n += 1;
    }
    if n == 0 {
        0.0
    } else {
        sum / f64::from(n)
    }
}

fn age_similarity(a: &str, b: &str) -> f64 {
    match (age_index(a), age_index(b)) {
        (Some(i), Some(j)) => match i.abs_diff(j) {
            0 => 1.0,
            1 => 0.5,
            _ => 0.0,
        },
        _ => {
            if canon_tag(a) == canon_tag(b) {
                1.0
            } else {
                0.0
            }
        }
    }
}

fn age_index(band: &str) -> Option<usize> {
    let mut n = band.trim().to_lowercase().replace('-', "_");
    if let Some(stripped) = n.strip_suffix('+') {
        n = format!("{stripped}plus");
    }
    match n.as_str() {
        "18_24" => Some(0),
        "25_34" => Some(1),
        "35_44" => Some(2),
        "45_54" => Some(3),
        "55_64" => Some(4),
        "65_plus" | "65plus" => Some(5),
        _ => None,
    }
}

fn sportiness_similarity(a: i16, b: i16) -> f64 {
    let a = f64::from(a.clamp(0, 3));
    let b = f64::from(b.clamp(0, 3));
    1.0 - (a - b).abs() / 3.0
}

fn jaccard(a: &[String], b: &[String]) -> Option<f64> {
    let a = tag_set(a);
    let b = tag_set(b);
    if a.is_empty() || b.is_empty() {
        return None;
    }
    let inter = a.intersection(&b).count();
    let union = a.len() + b.len() - inter;
    Some(inter as f64 / union as f64)
}

fn tag_set(tags: &[String]) -> HashSet<String> {
    tags.iter()
        .map(|tag| canon_tag(tag))
        .filter(|tag| !tag.is_empty())
        .collect()
}

fn shared_tags(likes: &[String], tags: &[String]) -> Vec<String> {
    let theirs = tag_set(tags);
    let mut shared: Vec<String> = likes
        .iter()
        .filter(|like| theirs.contains(&canon_tag(like)))
        .cloned()
        .collect();
    shared.sort_by(|a, b| canon_tag(a).cmp(&canon_tag(b)).then(a.cmp(b)));
    shared.dedup_by(|a, b| canon_tag(a) == canon_tag(b));
    shared
}

fn cosine(a: &[f32], b: &[f32]) -> f64 {
    if a.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let mut dot = 0.0;
    let mut na = 0.0;
    let mut nb = 0.0;
    for (x, y) in a.iter().zip(b.iter()) {
        let x = f64::from(*x);
        let y = f64::from(*y);
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 || !dot.is_finite() {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

fn event_fact_drop(event: &Candidate, now: DateTime<Utc>) -> bool {
    if event.status == EventStatus::Cancelled || !event.place_kind.is_public() {
        return true;
    }
    match event.starts_at.checked_add_signed(Duration::hours(24)) {
        Some(until) => until <= now,
        None => true,
    }
}

// Preference drops follow the person. An empty profile has none, so it skips
// them. Cancelled, expired, and non-public places are facts about the event
// and are dropped either way.
fn preference_drop(profile: &Profile, event: &Candidate) -> bool {
    if profile.is_empty() {
        return false;
    }
    if event.women_only && !profile.women_only {
        return true;
    }
    if mobility_conflict(profile.mobility.as_deref(), &event.activity_tags) {
        return true;
    }
    if dislike_hits(
        &profile.dislikes,
        &event.activity_tags,
        Some(event.title.as_str()),
    ) {
        return true;
    }
    if let Some(window) = profile.time_window {
        if !window_contains(window, warsaw_minute(event.starts_at)) {
            return true;
        }
    }
    false
}

fn person_preference_drop(profile: &Profile, person: &Person) -> bool {
    if profile.is_empty() {
        return false;
    }
    if person.women_only && !profile.women_only {
        return true;
    }
    if mobility_conflict(profile.mobility.as_deref(), &person.tags) {
        return true;
    }
    if dislike_hits(&profile.dislikes, &person.tags, None) {
        return true;
    }
    match (profile.time_window, person.time_window) {
        (Some(viewer), Some(theirs)) => !windows_overlap(viewer, theirs),
        _ => false,
    }
}

fn constraints_passed(profile: &Profile, person: &Person) -> Vec<String> {
    let mut passed = Vec::new();
    if profile.mobility.is_some() {
        passed.push("mobility".to_string());
    }
    if !profile.dislikes.is_empty() {
        passed.push("dislikes".to_string());
    }
    if person.women_only && profile.women_only {
        passed.push("women_only".to_string());
    }
    if profile.time_window.is_some() {
        passed.push("time_window".to_string());
    }
    passed
}

fn mobility_conflict(mobility: Option<&str>, tags: &[String]) -> bool {
    let Some(mobility) = mobility else {
        return false;
    };
    if !tokens(mobility).iter().any(|token| token == "wheelchair") {
        return false;
    }
    tags.iter()
        .any(|tag| tokens(tag).iter().any(|token| wheelchair_blocked(token)))
}

fn wheelchair_blocked(token: &str) -> bool {
    matches!(
        token,
        "padel"
            | "tennis"
            | "basketball"
            | "volleyball"
            | "squash"
            | "badminton"
            | "football"
            | "soccer"
            | "court"
            | "running"
            | "run"
    )
}

fn dislike_hits(dislikes: &[String], tags: &[String], title: Option<&str>) -> bool {
    dislikes.iter().any(|dislike| {
        let needles = tokens(dislike);
        if needles.is_empty() {
            return false;
        }
        let hit = |text: &str| {
            let hay = tokens(text);
            needles
                .iter()
                .any(|needle| hay.iter().any(|token| needle == token))
        };
        tags.iter().any(|tag| hit(tag)) || title.is_some_and(hit)
    })
}

fn tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|token| token.chars().count() >= 2)
        .map(|token| canonicalize_token(&token.to_lowercase()))
        .collect()
}

fn canonicalize_token(token: &str) -> String {
    match token {
        "paddle" => "padel".to_string(),
        other => other.to_string(),
    }
}

fn canon_tag(tag: &str) -> String {
    let tag = tag.trim().to_lowercase();
    match tag.as_str() {
        "paddle" => "padel".to_string(),
        other => other.to_string(),
    }
}

fn accept_location(
    origin: LatLng,
    radius_m: f64,
    bounds: Option<BBox>,
    lat: f64,
    lng: f64,
) -> Option<f64> {
    let distance_m = geo_distance(origin, lat, lng)?;
    if let Some(bounds) = bounds {
        in_bbox(lat, lng, bounds).then_some(distance_m)
    } else if distance_m <= radius_m {
        Some(distance_m)
    } else {
        None
    }
}

fn geo_distance(origin: LatLng, lat: f64, lng: f64) -> Option<f64> {
    if ![origin.lat, origin.lng, lat, lng]
        .into_iter()
        .all(|value| value.is_finite())
    {
        return None;
    }
    let distance_m = haversine_m(origin, lat, lng);
    distance_m.is_finite().then_some(distance_m)
}

fn in_bbox(lat: f64, lng: f64, bbox: BBox) -> bool {
    if lat < bbox.south || lat > bbox.north {
        return false;
    }
    if bbox.west <= bbox.east {
        lng >= bbox.west && lng <= bbox.east
    } else {
        lng >= bbox.west || lng <= bbox.east
    }
}

fn haversine_m(origin: LatLng, lat: f64, lng: f64) -> f64 {
    let lat1 = origin.lat.to_radians();
    let lat2 = lat.to_radians();
    let dlat = (lat - origin.lat).to_radians();
    let dlng = (lng - origin.lng).to_radians();
    let h = (dlat * 0.5).sin().powi(2) + lat1.cos() * lat2.cos() * (dlng * 0.5).sin().powi(2);
    2.0 * EARTH_RADIUS_M * h.clamp(0.0, 1.0).sqrt().asin()
}

fn distance_band(distance_m: f64) -> DistanceBand {
    if distance_m < 500.0 {
        DistanceBand::Within500M
    } else if distance_m < 2_000.0 {
        DistanceBand::Within2Km
    } else if distance_m < 5_000.0 {
        DistanceBand::Within5Km
    } else {
        DistanceBand::Farther
    }
}

// Profile windows are minutes from local midnight. Events are UTC; the city is Kraków.
fn warsaw_minute(t: DateTime<Utc>) -> u16 {
    let local = t + Duration::seconds(warsaw_offset_secs(t));
    u16::try_from(local.hour() * 60 + local.minute()).expect("minute of day")
}

fn warsaw_offset_secs(t: DateTime<Utc>) -> i64 {
    let year = t.date_naive().year();
    let start = warsaw_transition(year, 3);
    let end = warsaw_transition(year, 10);
    if t >= start && t < end {
        2 * 3600
    } else {
        3600
    }
}

fn warsaw_transition(year: i32, month: u32) -> DateTime<Utc> {
    last_sunday(year, month)
        .and_hms_opt(1, 0, 0)
        .expect("transition")
        .and_utc()
}

fn last_sunday(year: i32, month: u32) -> chrono::NaiveDate {
    let next = if month == 12 {
        chrono::NaiveDate::from_ymd_opt(year + 1, 1, 1)
    } else {
        chrono::NaiveDate::from_ymd_opt(year, month + 1, 1)
    }
    .expect("month");
    let last = next.pred_opt().expect("previous day");
    let back = i64::from(last.weekday().num_days_from_sunday());
    last - Duration::days(back)
}

fn window_parts(window: TimeWindow) -> [(u16, u16); 2] {
    let start = window.start_minute;
    let end = window.end_minute;
    if start > DAY_MINUTES || end > DAY_MINUTES {
        return [(0, 0), (0, 0)];
    }
    if start == end {
        return [(0, DAY_MINUTES), (0, 0)];
    }
    if start < end {
        [(start, end), (0, 0)]
    } else {
        [(start, DAY_MINUTES), (0, end)]
    }
}

fn window_contains(window: TimeWindow, minute: u16) -> bool {
    window_parts(window)
        .into_iter()
        .any(|(start, end)| minute >= start && minute < end)
}

fn windows_overlap(a: TimeWindow, b: TimeWindow) -> bool {
    let a = window_parts(a);
    let b = window_parts(b);
    a.iter()
        .any(|&(a0, a1)| b.iter().any(|&(b0, b1)| a0 < b1 && b0 < a1))
}

fn time_fit(window: Option<TimeWindow>, at: DateTime<Utc>) -> f64 {
    let Some(window) = window else {
        return 1.0;
    };
    let parts = window_parts(window);
    let len: u32 = parts
        .iter()
        .map(|(start, end)| u32::from(end - start))
        .sum();
    if len == 0 || len >= u32::from(DAY_MINUTES) {
        return 1.0;
    }
    let Some(offset) = offset_in_parts(&parts, warsaw_minute(at)) else {
        return 0.0;
    };
    let len = f64::from(len);
    let offset = f64::from(offset);
    1.0 - (offset - len / 2.0).abs() / len
}

fn offset_in_parts(parts: &[(u16, u16); 2], minute: u16) -> Option<u32> {
    let mut cursor = 0u32;
    for &(start, end) in parts {
        if minute >= start && minute < end {
            return Some(cursor + u32::from(minute - start));
        }
        cursor += u32::from(end - start);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    const ORIGIN: LatLng = LatLng {
        lat: 50.0,
        lng: 20.0,
    };

    fn at(year: i32, month: u32, day: u32, hour: u32, min: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(year, month, day, hour, min, 0)
            .unwrap()
    }

    fn close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
    }

    fn profile() -> Profile {
        Profile {
            age_band: None,
            gender: None,
            mobility: None,
            sportiness: None,
            likes: vec![],
            dislikes: vec![],
            women_only: false,
            time_window: None,
            embedding: None,
        }
    }

    fn event(id: u128, title: &str) -> Candidate {
        Candidate {
            id: Uuid::from_u128(id),
            title: title.to_string(),
            emoji: "x".to_string(),
            description: String::new(),
            activity_tags: vec![],
            women_only: false,
            starts_at: at(2026, 6, 15, 9, 0),
            capacity: None,
            promoted: false,
            status: EventStatus::Scheduled,
            place_name: "Park".to_string(),
            place_kind: PlaceKind::Park,
            latitude: ORIGIN.lat,
            longitude: ORIGIN.lng,
            host_name: "Ada".to_string(),
            signed_count: 0,
            embedding: None,
            attendees: vec![],
        }
    }

    fn attendee(age: &str) -> Attendee {
        Attendee {
            age_band: Some(age.to_string()),
            sportiness: None,
            tags: vec![],
            embedding: None,
            complements: false,
        }
    }

    fn person(id: u128, name: &str) -> Person {
        Person {
            id: Uuid::from_u128(id),
            first_name: name.to_string(),
            age_band: Some("25-34".to_string()),
            gender: None,
            mobility: None,
            sportiness: None,
            tags: vec!["walks".to_string()],
            women_only: false,
            time_window: None,
            embedding: Some(vec![1.0, 0.0]),
            latitude: ORIGIN.lat,
            longitude: ORIGIN.lng,
            complements: false,
        }
    }

    fn run(
        profile: &Profile,
        events: &[Candidate],
        radius_m: f64,
        bounds: Option<BBox>,
        now: DateTime<Utc>,
    ) -> Vec<ScoredEvent> {
        rank_events(&RankInput {
            profile,
            origin: ORIGIN,
            radius_m,
            bounds,
            now,
            want_embedding: None,
            events,
        })
        .unwrap()
    }

    fn run_people(profile: &Profile, people: &[Person]) -> Vec<PersonHit> {
        rank_people(&PeopleInput {
            profile,
            origin: ORIGIN,
            now: at(2026, 6, 15, 8, 0),
            people,
        })
        .unwrap()
    }

    #[test]
    fn dislike_drops_the_row() {
        let mut profile = profile();
        profile.dislikes = vec!["paddle sports".to_string()];
        let mut tagged = event(1, "Morning game");
        tagged.activity_tags = vec!["padel".to_string()];
        let titled = event(2, "Evening paddle");
        let kept = event(3, "Chess in the park");
        kept_ids(
            &run(
                &profile,
                &[tagged, titled, kept],
                30_000.0,
                None,
                at(2026, 6, 15, 8, 0),
            ),
            &[3],
        );
    }

    #[test]
    fn mobility_conflict_drops_the_row() {
        let mut profile = profile();
        profile.mobility = Some("wheelchair".to_string());
        let mut court = event(1, "Padel");
        court.activity_tags = vec!["padel".to_string()];
        let mut walk = event(2, "Park walk");
        walk.activity_tags = vec!["walk".to_string()];
        kept_ids(
            &run(
                &profile,
                &[court, walk],
                30_000.0,
                None,
                at(2026, 6, 15, 8, 0),
            ),
            &[2],
        );
    }

    #[test]
    fn women_only_drops_when_the_profile_excludes_it() {
        let mut profile = profile();
        profile.age_band = Some("25-34".to_string());
        let mut closed = event(1, "Women's walk");
        closed.women_only = true;
        let open = event(2, "Park walk");
        let now = at(2026, 6, 15, 8, 0);
        kept_ids(
            &run(
                &profile,
                &[closed.clone(), open.clone()],
                30_000.0,
                None,
                now,
            ),
            &[2],
        );

        profile.women_only = true;
        kept_ids(
            &run(&profile, &[closed, open], 30_000.0, None, now),
            &[1, 2],
        );
    }

    #[test]
    fn cold_profile_keeps_a_disliked_looking_event_and_orders_by_soonest_start() {
        let profile = profile();
        assert!(profile.is_empty());
        let now = at(2026, 6, 15, 8, 0);
        let mut soon_far = event(1, "Padel night");
        soon_far.activity_tags = vec!["padel".to_string()];
        soon_far.starts_at = now + Duration::hours(1);
        soon_far.latitude = 50.15;
        let mut soon_near = event(2, "Padel night");
        soon_near.activity_tags = vec!["padel".to_string()];
        soon_near.starts_at = soon_far.starts_at;
        let mut later_near = event(3, "Padel night");
        later_near.activity_tags = vec!["padel".to_string()];
        later_near.starts_at = now + Duration::hours(5);
        let hits = run(
            &profile,
            &[later_near, soon_far, soon_near],
            30_000.0,
            None,
            now,
        );
        assert_eq!(
            hits.iter().map(|hit| hit.id).collect::<Vec<_>>(),
            vec![Uuid::from_u128(2), Uuid::from_u128(1), Uuid::from_u128(3)]
        );
        assert!(hits.iter().all(|hit| hit.score == 0.0));
    }

    #[test]
    fn promoted_cannot_beat_a_hard_drop() {
        let mut profile = profile();
        profile.dislikes = vec!["padel".to_string()];
        let mut promoted = event(1, "Padel");
        promoted.activity_tags = vec!["padel".to_string()];
        promoted.promoted = true;
        let mut plain = event(2, "Walk");
        plain.activity_tags = vec!["walk".to_string()];
        plain.latitude = 50.1;
        let hits = run(
            &profile,
            &[promoted, plain],
            30_000.0,
            None,
            at(2026, 6, 15, 8, 0),
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, Uuid::from_u128(2));
        assert!(!hits[0].promoted);
    }

    #[test]
    fn promoted_bonus_is_at_most_005_and_does_not_resurrect_a_drop() {
        const { assert!(PROMOTED_BONUS <= 0.05) };
        let mut profile = profile();
        profile.age_band = Some("25-34".to_string());
        let mut promoted = event(1, "Walk");
        promoted.promoted = true;
        let plain = event(2, "Walk");
        let mut cancelled = event(3, "Walk");
        cancelled.promoted = true;
        cancelled.status = EventStatus::Cancelled;
        let hits = run(
            &profile,
            &[plain, cancelled, promoted],
            30_000.0,
            None,
            at(2026, 6, 15, 8, 0),
        );
        assert_eq!(hits.len(), 2);
        assert!(hits[0].promoted);
        assert!(!hits[1].promoted);
        close(hits[0].score - hits[1].score, PROMOTED_BONUS);
        assert!(hits[0].score - hits[1].score <= 0.05 + 1e-12);
    }

    #[test]
    fn no_attendees_renormalizes_so_nearer_beats_farther() {
        let mut profile = profile();
        profile.age_band = Some("25-34".to_string());
        let near = event(1, "Near walk");
        let mut far = event(2, "Far walk");
        far.longitude = 20.1;
        let hits = run(
            &profile,
            &[far, near],
            30_000.0,
            None,
            at(2026, 6, 15, 8, 0),
        );
        assert_eq!(hits[0].id, Uuid::from_u128(1));
        assert!(hits[0].score > hits[1].score);
        let expected = (DISTANCE_WEIGHT + TIME_WEIGHT) / (DISTANCE_WEIGHT + TIME_WEIGHT);
        close(hits[0].score, expected);
    }

    #[test]
    fn want_vector_reorders_events_and_a_missing_vector_is_not_zero() {
        let mut profile = profile();
        profile.age_band = Some("25-34".to_string());
        let mut chess = event(1, "Chess");
        chess.embedding = Some(vec![1.0, 0.0]);
        let mut walk = event(2, "Walk");
        walk.embedding = Some(vec![0.0, 1.0]);
        let bare = event(3, "Bare");
        let now = at(2026, 6, 15, 8, 0);
        let toward_chess = rank_events(&RankInput {
            profile: &profile,
            origin: ORIGIN,
            radius_m: 30_000.0,
            bounds: None,
            now,
            want_embedding: Some(&[1.0, 0.0]),
            events: &[walk.clone(), bare.clone(), chess.clone()],
        })
        .unwrap();
        assert_eq!(toward_chess[0].id, Uuid::from_u128(1));
        let toward_walk = rank_events(&RankInput {
            profile: &profile,
            origin: ORIGIN,
            radius_m: 30_000.0,
            bounds: None,
            now,
            want_embedding: Some(&[0.0, 1.0]),
            events: &[chess, walk, bare],
        })
        .unwrap();
        assert_eq!(toward_walk[0].id, Uuid::from_u128(2));
        assert!(toward_walk.iter().any(|hit| hit.id == Uuid::from_u128(3)));
        let orthogonal = toward_chess
            .iter()
            .find(|hit| hit.id == Uuid::from_u128(2))
            .unwrap()
            .score;
        let missing = toward_chess
            .iter()
            .find(|hit| hit.id == Uuid::from_u128(3))
            .unwrap()
            .score;
        assert!(missing > orthogonal);
    }

    #[test]
    fn company_complement_is_a_rule() {
        let lonely = vec!["samotna".to_string()];
        let company = vec!["towarzystwo".to_string()];
        let coffee = vec!["coffee".to_string()];
        let wants = vec![("social".to_string(), "wants company".to_string())];
        let alone = vec![("home".to_string(), "lives alone".to_string())];
        assert!(company_complements(&lonely, &[], &company, &[]));
        assert!(company_complements(&[], &alone, &[], &wants));
        assert!(company_complements(
            &["towarzystwo".to_string(), "coffee".to_string()],
            &[],
            &["wants company".to_string(), "coffee".to_string()],
            &[],
        ));
        assert!(!company_complements(&lonely, &[], &lonely, &[]));
        assert!(!company_complements(&company, &[], &coffee, &[]));
        assert!(!company_complements(&coffee, &[], &coffee, &[]));
    }

    #[test]
    fn viewport_radius_matches_formula_and_clamps() {
        let lat: f64 = 50.0647;
        let zoom = 14.0;
        let raw =
            VIEWPORT_METERS_PER_PIXEL * lat.to_radians().cos() / 2f64.powf(zoom) * VIEWPORT_PIXELS;
        let expected = raw.clamp(MIN_RADIUS_M, MAX_RADIUS_M);
        close(viewport_radius_m(lat, zoom).unwrap(), expected);
        assert!(expected > MIN_RADIUS_M && expected < MAX_RADIUS_M);
        assert_eq!(viewport_radius_m(0.0, 0.0).unwrap(), MAX_RADIUS_M);
        assert_eq!(viewport_radius_m(0.0, 22.0).unwrap(), MIN_RADIUS_M);
        assert_eq!(viewport_radius_m(180.0, 10.0).unwrap(), MIN_RADIUS_M);
        assert!(viewport_radius_m(f64::NAN, 1.0).is_err());
        assert!(viewport_radius_m(1.0, f64::INFINITY).is_err());
        assert!(viewport_radius_m(f64::NEG_INFINITY, 1.0).is_err());
    }

    #[test]
    fn bbox_beats_radius() {
        let parsed = parse_bbox("19, 49, 21, 52").unwrap();
        assert_eq!(
            parsed,
            BBox {
                west: 19.0,
                south: 49.0,
                east: 21.0,
                north: 52.0,
            }
        );
        assert!(parse_bbox("19,49,21").is_err());
        assert!(parse_bbox("19,49,21,52,0").is_err());
        assert!(parse_bbox("19,49,21,nan").is_err());
        assert!(parse_bbox("a,b,c,d").is_err());
        assert!(parse_bbox("").is_err());

        let profile = profile();
        let now = at(2026, 6, 15, 8, 0);
        let mut far = event(1, "Far");
        far.latitude = 50.5;
        assert!(run(&profile, &[far.clone()], 1_000.0, None, now).is_empty());
        assert_eq!(
            run(&profile, &[far], 1_000.0, Some(parsed), now)[0].id,
            Uuid::from_u128(1)
        );

        let tight = BBox {
            west: 30.0,
            south: 40.0,
            east: 31.0,
            north: 41.0,
        };
        let near = event(2, "Near");
        assert!(run(
            &profile,
            std::slice::from_ref(&near),
            30_000.0,
            Some(tight),
            now
        )
        .is_empty());
        assert_eq!(
            run(&profile, &[near], 30_000.0, None, now)[0].id,
            Uuid::from_u128(2)
        );
    }

    #[test]
    fn cancelled_and_expired_drop_even_for_an_empty_profile() {
        let profile = profile();
        assert!(profile.is_empty());
        let now = at(2026, 6, 15, 12, 0);
        let mut cancelled = event(1, "Cancelled");
        cancelled.status = EventStatus::Cancelled;
        cancelled.starts_at = now + Duration::hours(2);
        let mut expired = event(2, "Expired");
        expired.starts_at = now - Duration::hours(24);
        let mut barely = event(3, "Barely");
        barely.starts_at = now - Duration::hours(24) + Duration::seconds(1);
        let mut private = event(4, "Home");
        private.place_kind = PlaceKind::NotPublic;
        private.starts_at = now + Duration::hours(1);
        let mut live = event(5, "Live");
        live.starts_at = now + Duration::hours(1);
        kept_ids(
            &run(
                &profile,
                &[cancelled, expired, barely, private, live],
                30_000.0,
                None,
                now,
            ),
            &[3, 5],
        );
    }

    #[test]
    fn similarity_averages_only_parts_that_exist() {
        let mut profile = profile();
        profile.age_band = Some("25-34".to_string());
        profile.sportiness = Some(0);
        profile.likes = vec!["walks".to_string(), "coffee".to_string()];
        profile.embedding = Some(vec![1.0, 0.0]);

        close(
            similarity(&profile, Some("25-34"), None, &[], None, false),
            1.0,
        );
        close(
            similarity(&profile, Some("35-44"), None, &[], None, false),
            0.5,
        );
        close(
            similarity(&profile, Some("65+"), None, &[], None, false),
            0.0,
        );
        close(
            similarity(&profile, Some("25_34"), None, &[], None, false),
            1.0,
        );
        close(
            similarity(&profile, Some("custom"), None, &[], None, false),
            0.0,
        );
        let mut custom = profile.clone();
        custom.age_band = Some("custom".to_string());
        close(
            similarity(&custom, Some("custom"), None, &[], None, false),
            1.0,
        );

        close(
            similarity(&profile, Some("25-34"), Some(3), &[], None, false),
            0.5,
        );
        close(
            similarity(&profile, Some("25-34"), Some(0), &[], None, false),
            1.0,
        );
        close(sportiness_similarity(1, 2), 2.0 / 3.0);
        close(sportiness_similarity(-1, 9), 0.0);

        let walks = vec!["walks".to_string()];
        close(
            similarity(&profile, Some("25-34"), None, &walks, None, false),
            (1.0 + 0.5) / 2.0,
        );
        close(similarity(&profile, None, None, &[], None, false), 0.0);
        close(
            similarity(&profile, Some("25-34"), None, &[], Some(&[0.0, 0.0]), false),
            0.5,
        );
        close(
            similarity(&profile, Some("25-34"), None, &[], Some(&[1.0, 0.0]), false),
            1.0,
        );
        close(
            similarity(&profile, Some("65+"), None, &[], None, true),
            0.5,
        );
        close(similarity(&profile, None, None, &[], None, false), 0.0);
    }

    #[test]
    fn score_uses_mean_attendee_similarity() {
        let mut profile = profile();
        profile.age_band = Some("25-34".to_string());
        let mut one = event(1, "One");
        one.attendees = vec![attendee("25-34")];
        let mut two = event(2, "Two");
        two.attendees = vec![attendee("25-34"), attendee("65+")];
        let hits = run(&profile, &[two, one], 30_000.0, None, at(2026, 6, 15, 8, 0));
        let by_id = |id: u128| {
            hits.iter()
                .find(|hit| hit.id == Uuid::from_u128(id))
                .unwrap()
                .score
        };
        let used = PEOPLE_WEIGHT + DISTANCE_WEIGHT + TIME_WEIGHT;
        close(by_id(1), used / used);
        close(
            by_id(2),
            (PEOPLE_WEIGHT * 0.5 + DISTANCE_WEIGHT + TIME_WEIGHT) / used,
        );
    }

    #[test]
    fn time_window_is_warsaw_local_and_wraps() {
        let mut profile = profile();
        profile.time_window = Some(TimeWindow {
            start_minute: 10 * 60,
            end_minute: 12 * 60,
        });
        let now = at(2026, 6, 15, 0, 0);
        let mut start = event(1, "Start");
        start.starts_at = at(2026, 6, 15, 8, 0);
        let mut late = event(2, "Late");
        late.starts_at = at(2026, 6, 15, 9, 59);
        let mut end = event(3, "End");
        end.starts_at = at(2026, 6, 15, 10, 0);
        let mut early = event(4, "Early");
        early.starts_at = at(2026, 6, 15, 6, 0);
        kept_ids(
            &run(&profile, &[start, late, end, early], 30_000.0, None, now),
            &[1, 2],
        );

        profile.time_window = Some(TimeWindow {
            start_minute: 22 * 60,
            end_minute: 2 * 60,
        });
        let now = at(2026, 1, 15, 0, 0);
        let mut evening = event(1, "Evening");
        evening.starts_at = at(2026, 1, 15, 21, 0);
        let mut after_midnight = event(2, "After midnight");
        after_midnight.starts_at = at(2026, 1, 15, 0, 59);
        let mut past_end = event(3, "Past end");
        past_end.starts_at = at(2026, 1, 15, 1, 0);
        kept_ids(
            &run(
                &profile,
                &[evening, after_midnight, past_end],
                30_000.0,
                None,
                now,
            ),
            &[2, 1],
        );
    }

    #[test]
    fn haversine_uses_earth_radius_6371000() {
        let distance = haversine_m(LatLng { lat: 0.0, lng: 0.0 }, 1.0, 0.0);
        let expected = 6_371_000.0 * std::f64::consts::PI / 180.0;
        assert!((distance - expected).abs() < 1e-4);
    }

    #[test]
    fn rank_people_applies_hard_drops_then_cosine() {
        let mut profile = profile();
        profile.age_band = Some("25-34".to_string());
        profile.mobility = Some("wheelchair".to_string());
        profile.dislikes = vec!["golf".to_string()];
        profile.likes = vec!["walks".to_string()];
        profile.embedding = Some(vec![1.0, 0.0]);

        let anna = person(1, "Anna");
        let mut beata = person(2, "Beata");
        beata.embedding = Some(vec![0.0, 1.0]);
        beata.longitude = east_of(3_000.0);
        let mut cela = person(3, "Cela");
        cela.tags = vec!["padel".to_string()];
        let mut dora = person(4, "Dora");
        dora.women_only = true;
        let mut ewa = person(5, "Ewa");
        ewa.tags = vec!["golf".to_string()];

        let hits = run_people(&profile, &[beata, cela, anna, dora, ewa]);
        assert_eq!(
            hits.iter()
                .map(|hit| hit.first_name.as_str())
                .collect::<Vec<_>>(),
            vec!["Anna", "Beata"]
        );
        assert_eq!(hits[0].age_band.as_deref(), Some("25-34"));
        assert_eq!(hits[0].shared_tags, vec!["walks".to_string()]);
        assert_eq!(hits[0].distance_band, DistanceBand::Within500M);
        assert_eq!(
            hits[0].constraints_passed,
            vec!["mobility".to_string(), "dislikes".to_string()]
        );
        assert_eq!(hits[1].distance_band, DistanceBand::Within5Km);

        assert_eq!(band_at(0.0), DistanceBand::Within500M);
        assert_eq!(band_at(1_000.0), DistanceBand::Within2Km);
        assert_eq!(band_at(3_000.0), DistanceBand::Within5Km);
        assert_eq!(band_at(10_000.0), DistanceBand::Farther);

        profile.women_only = true;
        profile.time_window = Some(TimeWindow {
            start_minute: 10 * 60,
            end_minute: 12 * 60,
        });
        let mut overlap = person(6, "Overlap");
        overlap.women_only = true;
        overlap.time_window = Some(TimeWindow {
            start_minute: 11 * 60,
            end_minute: 13 * 60,
        });
        let mut apart = person(7, "Apart");
        apart.time_window = Some(TimeWindow {
            start_minute: 13 * 60,
            end_minute: 15 * 60,
        });
        let flexible = person(8, "Flexible");
        let hits = run_people(&profile, &[apart, flexible, overlap]);
        assert_eq!(
            hits.iter()
                .map(|hit| hit.first_name.as_str())
                .collect::<Vec<_>>(),
            vec!["Overlap", "Flexible"]
        );
        assert!(hits[0]
            .constraints_passed
            .iter()
            .any(|name| name == "women_only"));
        assert!(hits[0]
            .constraints_passed
            .iter()
            .any(|name| name == "time_window"));
    }

    fn kept_ids(hits: &[ScoredEvent], ids: &[u128]) {
        let mut got: Vec<u128> = hits.iter().map(|hit| hit.id.as_u128()).collect();
        let mut expect = ids.to_vec();
        got.sort_unstable();
        expect.sort_unstable();
        assert_eq!(got, expect);
    }

    fn east_of(meters: f64) -> f64 {
        let meters_per_deg =
            6_371_000.0 * ORIGIN.lat.to_radians().cos() * std::f64::consts::PI / 180.0;
        ORIGIN.lng + meters / meters_per_deg
    }

    fn band_at(meters: f64) -> DistanceBand {
        let mut someone = person(9, "Band");
        someone.longitude = if meters == 0.0 {
            ORIGIN.lng
        } else {
            east_of(meters)
        };
        let hits = run_people(&profile(), &[someone]);
        assert_eq!(hits.len(), 1);
        if meters > 0.0 {
            let distance = haversine_m(ORIGIN, ORIGIN.lat, east_of(meters));
            assert!((distance - meters).abs() / meters < 0.02);
        }
        hits[0].distance_band
    }
}
