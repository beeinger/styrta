use std::time::Duration;

use async_trait::async_trait;
use reqwest::header::USER_AGENT;
use serde::Deserialize;

use crate::config::Config;
use crate::rank::PlaceKind;

const TIMEOUT: Duration = Duration::from_secs(8);
const LIMIT: usize = 3;
/// Małopolska, Nominatim viewbox order: west, north, east, south.
const WEST: f64 = 19.05;
const NORTH: f64 = 50.52;
const EAST: f64 = 21.50;
const SOUTH: f64 = 49.15;

#[derive(Clone, Debug, PartialEq)]
pub struct PlaceHit {
    pub name: String,
    pub street: String,
    pub city: String,
    pub address: String,
    pub latitude: f64,
    pub longitude: f64,
    pub kind: PlaceKind,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlaceSearch {
    pub places: Vec<PlaceHit>,
    pub rejected_private: bool,
}

#[derive(Debug)]
pub enum Error {
    EmptyQuery,
    Transport(String),
    Status(u16),
    Body,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyQuery => f.write_str("empty place query"),
            Self::Transport(_) => f.write_str("place search failed"),
            Self::Status(code) => write!(f, "place search status {code}"),
            Self::Body => f.write_str("place search failed"),
        }
    }
}

impl std::error::Error for Error {}

#[async_trait]
pub trait Geocoder: Send + Sync {
    async fn search(&self, query: &str) -> Result<PlaceSearch, Error>;
}

#[derive(Clone, Debug)]
pub struct Client {
    http: reqwest::Client,
    base: String,
    user_agent: String,
}

impl Client {
    pub fn new(cfg: &Config) -> Result<Self, Error> {
        let http = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|err| Error::Transport(err.to_string()))?;
        Ok(Self {
            http,
            base: cfg.geocoder_base_url.clone(),
            user_agent: cfg.geocoder_user_agent.clone(),
        })
    }
}

#[async_trait]
impl Geocoder for Client {
    async fn search(&self, query: &str) -> Result<PlaceSearch, Error> {
        let query = query.trim();
        if query.is_empty() {
            return Err(Error::EmptyQuery);
        }
        let query: String = query.chars().take(180).collect();
        let viewbox = format!("{WEST},{NORTH},{EAST},{SOUTH}");
        let response = self
            .http
            .get(format!("{}/search", self.base))
            .header(USER_AGENT, &self.user_agent)
            .query(&[
                ("format", "jsonv2"),
                ("addressdetails", "1"),
                ("limit", "5"),
                ("countrycodes", "pl"),
                ("bounded", "1"),
                ("accept-language", "pl"),
                ("viewbox", viewbox.as_str()),
                ("q", query.as_str()),
            ])
            .send()
            .await
            .map_err(|err| Error::Transport(err.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            return Err(Error::Status(status.as_u16()));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|err| Error::Transport(err.to_string()))?;
        parse_results(&bytes)
    }
}

pub fn parse_results(bytes: &[u8]) -> Result<PlaceSearch, Error> {
    let items: Vec<RawPlace> = serde_json::from_slice(bytes).map_err(|_| Error::Body)?;
    let mut places = Vec::new();
    let mut rejected_private = false;
    for item in items {
        match item.into_hit() {
            HitClass::Public(hit) => {
                if places.len() < LIMIT {
                    places.push(hit);
                }
            }
            HitClass::Private => rejected_private = true,
            HitClass::Skip => {}
        }
    }
    Ok(PlaceSearch {
        rejected_private: rejected_private && places.is_empty(),
        places,
    })
}

enum HitClass {
    Public(PlaceHit),
    Private,
    Skip,
}

#[derive(Deserialize)]
struct RawPlace {
    lat: String,
    lon: String,
    category: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    address: RawAddress,
}

impl RawPlace {
    fn into_hit(self) -> HitClass {
        let Some(kind) = public_kind(&self.category, &self.kind) else {
            return HitClass::Private;
        };
        let Ok(latitude) = self.lat.trim().parse::<f64>() else {
            return HitClass::Skip;
        };
        let Ok(longitude) = self.lon.trim().parse::<f64>() else {
            return HitClass::Skip;
        };
        if !in_region(latitude, longitude) {
            return HitClass::Skip;
        }
        let name = place_name(&self.name, &self.address, &self.display_name);
        if name.is_empty() {
            return HitClass::Skip;
        }
        let street = street(&self.address);
        let city = city(&self.address);
        let address = address_line(&street, &city, &self.display_name);
        HitClass::Public(PlaceHit {
            name,
            street,
            city,
            address,
            latitude,
            longitude,
            kind,
        })
    }
}

#[derive(Default, Deserialize)]
struct RawAddress {
    #[serde(default)]
    house_number: String,
    #[serde(default)]
    road: String,
    #[serde(default)]
    pedestrian: String,
    #[serde(default)]
    city: String,
    #[serde(default)]
    town: String,
    #[serde(default)]
    village: String,
    #[serde(default)]
    municipality: String,
    #[serde(default)]
    amenity: String,
    #[serde(default)]
    shop: String,
    #[serde(default)]
    tourism: String,
    #[serde(default)]
    leisure: String,
}

fn public_kind(category: &str, kind: &str) -> Option<PlaceKind> {
    match (category, kind) {
        (
            "building",
            "house" | "residential" | "apartments" | "detached" | "semidetached_house"
            | "terrace" | "bungalow" | "cabin" | "farm" | "static_caravan" | "yes",
        ) => None,
        ("place", "house" | "isolated_dwelling" | "farm") => None,
        ("highway", _) | ("boundary", _) => None,
        (
            "amenity",
            "cafe" | "restaurant" | "bar" | "pub" | "fast_food" | "ice_cream" | "biergarten"
            | "food_court",
        ) => Some(PlaceKind::Cafe),
        ("leisure", "park" | "garden" | "nature_reserve")
        | ("natural", "wood" | "grassland" | "scrub")
        | ("landuse", "forest" | "grass" | "recreation_ground" | "meadow") => {
            Some(PlaceKind::Park)
        }
        (
            "amenity",
            "community_centre" | "theatre" | "arts_centre" | "library" | "cinema"
            | "conference_centre" | "events_venue" | "social_facility",
        )
        | ("building", "civic" | "public")
        | ("leisure", "stadium" | "sports_centre" | "sports_hall") => Some(PlaceKind::Hall),
        ("place", "square") | ("leisure", "pitch") => Some(PlaceKind::Square),
        _ => Some(PlaceKind::OtherPublic),
    }
}

fn place_name(name: &str, address: &RawAddress, display_name: &str) -> String {
    for candidate in [
        name,
        address.amenity.as_str(),
        address.shop.as_str(),
        address.tourism.as_str(),
        address.leisure.as_str(),
    ] {
        let candidate = candidate.trim();
        if !candidate.is_empty() {
            return candidate.to_string();
        }
    }
    display_name
        .split(',')
        .next()
        .unwrap_or("")
        .trim()
        .to_string()
}

fn street(address: &RawAddress) -> String {
    let road = if address.road.trim().is_empty() {
        address.pedestrian.trim()
    } else {
        address.road.trim()
    };
    let number = address.house_number.trim();
    match (road.is_empty(), number.is_empty()) {
        (true, _) => String::new(),
        (false, true) => road.to_string(),
        (false, false) => format!("{road} {number}"),
    }
}

fn city(address: &RawAddress) -> String {
    for candidate in [
        address.city.as_str(),
        address.town.as_str(),
        address.village.as_str(),
        address.municipality.as_str(),
    ] {
        let candidate = candidate.trim();
        if !candidate.is_empty() {
            return candidate.to_string();
        }
    }
    String::new()
}

fn address_line(street: &str, city: &str, display_name: &str) -> String {
    match (street.is_empty(), city.is_empty()) {
        (false, false) => format!("{street}, {city}"),
        (false, true) => street.to_string(),
        (true, false) => city.to_string(),
        (true, true) => display_name.trim().to_string(),
    }
}

fn in_region(latitude: f64, longitude: f64) -> bool {
    (SOUTH..=NORTH).contains(&latitude) && (WEST..=EAST).contains(&longitude)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_arena_and_drops_a_house() {
        let body = r#"[
            {
                "lat": "50.0677202",
                "lon": "19.9915490",
                "category": "leisure",
                "type": "stadium",
                "name": "Tauron Arena Kraków",
                "display_name": "Tauron Arena Kraków, 7, Stanisława Lema, Czyżyny, Kraków, Polska",
                "address": {
                    "leisure": "Tauron Arena Kraków",
                    "house_number": "7",
                    "road": "Stanisława Lema",
                    "city": "Kraków"
                }
            },
            {
                "lat": "50.06",
                "lon": "19.94",
                "category": "building",
                "type": "house",
                "name": "Dom",
                "display_name": "Dom, 1, Cicha, Kraków",
                "address": {"house_number": "1", "road": "Cicha", "city": "Kraków"}
            }
        ]"#;
        let found = parse_results(body.as_bytes()).unwrap();
        assert!(!found.rejected_private);
        assert_eq!(found.places.len(), 1);
        let arena = &found.places[0];
        assert_eq!(arena.name, "Tauron Arena Kraków");
        assert_eq!(arena.street, "Stanisława Lema 7");
        assert_eq!(arena.city, "Kraków");
        assert_eq!(arena.address, "Stanisława Lema 7, Kraków");
        assert_eq!(arena.kind, PlaceKind::Hall);
        assert!((arena.latitude - 50.0677202).abs() < 1e-9);
        assert!((arena.longitude - 19.9915490).abs() < 1e-9);
    }

    #[test]
    fn a_house_alone_is_private() {
        let body = r#"[{
            "lat": "50.06",
            "lon": "19.94",
            "category": "building",
            "type": "apartments",
            "name": "Blok",
            "display_name": "Blok, Kraków",
            "address": {"city": "Kraków"}
        }]"#;
        let found = parse_results(body.as_bytes()).unwrap();
        assert!(found.places.is_empty());
        assert!(found.rejected_private);
    }

    #[test]
    fn drops_a_hit_outside_malopolska() {
        let body = r#"[{
            "lat": "52.23",
            "lon": "21.01",
            "category": "amenity",
            "type": "cafe",
            "name": "Warsaw Cafe",
            "display_name": "Warsaw Cafe",
            "address": {"road": "Nowy Świat", "city": "Warszawa"}
        }]"#;
        let found = parse_results(body.as_bytes()).unwrap();
        assert!(found.places.is_empty());
        assert!(!found.rejected_private);
    }

    #[test]
    fn an_error_object_is_not_an_empty_list() {
        let err = parse_results(br#"{"error":"Unable to geocode"}"#).unwrap_err();
        assert!(err.to_string().contains("failed"));
    }
}
