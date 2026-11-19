//! GraphQL "pathfinder" backends (search, Home) — the API the desktop and
//! web clients use for browse data.
//!
//! Requests are persisted queries: `GET /pathfinder/v1/query?operationName=
//! <op>&variables=<json>&extensions={"persistedQuery":{"version":1,
//! "sha256Hash":"<hash>"}}`. Spotify rotates these hashes and reshapes the
//! responses when the clients update, so:
//! * every hash lives in [`Queries`] (this module only) and can be
//!   overridden without a rebuild from `~/.config/ferrotune/pathfinder.json`
//!   (`{"searchDesktop": "<sha256>"}`), see docs/protocol.md "Refreshing
//!   persisted queries";
//! * a rejected hash (`PersistedQueryNotFound`, HTTP 400/404/412) becomes
//!   [`Error::QueryRejected`], and each backend then falls back to an
//!   spclient endpoint so the screen still works;
//! * responses are parsed leniently (`net::types::gql_*`).

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use serde_json::{Value, json};

use super::backend::{HomeApi, SearchApi};
use super::client::{InternalClient, JSON, Request, Target};
use super::model::{Card, HomeFeed, SearchResults, Shelf};
use super::spclient::Internal;
use super::types;
use crate::error::{Error, Result};

pub const TTL_SEARCH: Duration = Duration::from_secs(10 * 60);
pub const TTL_HOME: Duration = Duration::from_secs(30 * 60);

pub const OP_SEARCH: &str = "searchDesktop";
pub const OP_HOME: &str = "home";

/// Last known persisted-query hashes. UNVERIFIED against the live service in
/// this repository (no account in CI): they are the values public desktop/web
/// client builds were seen sending, and they *will* go stale. See
/// docs/protocol.md for how to refresh them.
const DEFAULT_HASHES: &[(&str, &str)] = &[
    (
        OP_SEARCH,
        "21969b655b795601fb2d2204a4243188e75fdc6d3520e7b9cd3f4db2aff9591e",
    ),
    (
        OP_HOME,
        "eb3fba2d388cf4fc4d696b1757a58584e9538a3b515ea742e9cc9465807340be",
    ),
];

/// Operation name -> sha256 hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Queries {
    hashes: HashMap<String, String>,
}

impl Default for Queries {
    fn default() -> Self {
        Self {
            hashes: DEFAULT_HASHES
                .iter()
                .map(|(op, h)| ((*op).to_owned(), (*h).to_owned()))
                .collect(),
        }
    }
}

fn is_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

impl Queries {
    /// Defaults with overrides from a JSON object `{"<op>": "<sha256>"}`.
    /// Invalid entries are ignored (and logged).
    pub fn with_overrides(json_text: &str) -> Self {
        let mut q = Self::default();
        match serde_json::from_str::<HashMap<String, String>>(json_text) {
            Ok(map) => {
                for (op, hash) in map {
                    if is_sha256(&hash) {
                        q.hashes.insert(op, hash.to_ascii_lowercase());
                    } else {
                        tracing::warn!(op, "ignoring invalid persisted-query hash override");
                    }
                }
            }
            Err(e) => tracing::warn!(error = %e, "ignoring invalid pathfinder.json"),
        }
        q
    }

    pub fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::with_overrides(&text),
            Err(_) => Self::default(),
        }
    }

    pub fn hash(&self, op: &str) -> Option<&str> {
        self.hashes.get(op).map(String::as_str)
    }

    /// Path + query for a persisted query.
    pub fn path(&self, op: &str, variables: &Value) -> Result<String> {
        let hash = self
            .hash(op)
            .ok_or_else(|| Error::QueryRejected(op.to_owned()))?;
        let ext = json!({"persistedQuery": {"version": 1, "sha256Hash": hash}});
        let mut q = url::form_urlencoded::Serializer::new(String::new());
        q.append_pair("operationName", op)
            .append_pair("variables", &variables.to_string())
            .append_pair("extensions", &ext.to_string());
        Ok(format!("/pathfinder/v1/query?{}", q.finish()))
    }
}

/// GraphQL errors array -> rejected query / protocol error.
pub fn check_errors(op: &str, v: &Value) -> Result<()> {
    let Some(errors) = v.get("errors").and_then(Value::as_array) else {
        return Ok(());
    };
    if errors.is_empty() {
        return Ok(());
    }
    let rejected = errors.iter().any(|e| {
        let msg = e.get("message").and_then(Value::as_str).unwrap_or("");
        msg.contains("PersistedQueryNotFound") || msg.contains("PersistedQueryNotSupported")
    });
    if rejected || v.get("data").is_none_or(Value::is_null) {
        return Err(Error::QueryRejected(op.to_owned()));
    }
    // Partial data with errors: use what we got.
    Ok(())
}

/// Only complete GraphQL answers are cached: a 200 with `errors` or null
/// `data` (rejected hash, partial outage) must not be served for the TTL.
pub fn cacheable(body: &[u8]) -> bool {
    let Ok(v) = serde_json::from_slice::<Value>(body) else {
        return false;
    };
    let no_errors = v
        .get("errors")
        .and_then(Value::as_array)
        .is_none_or(Vec::is_empty);
    no_errors && v.get("data").is_some_and(|d| !d.is_null())
}

/// Should a pathfinder failure fall back to the spclient endpoint?
/// Everything except being offline or in a rate-limit cooldown, where a
/// second request would not help either.
pub fn should_fall_back(e: &Error) -> bool {
    !matches!(e, Error::Net(_) | Error::RateLimited(_) | Error::Cancelled)
}

pub struct Pathfinder {
    client: Arc<InternalClient>,
    queries: Queries,
    fallback: Arc<Internal>,
}

impl Pathfinder {
    pub fn new(client: Arc<InternalClient>, queries: Queries, fallback: Arc<Internal>) -> Self {
        Self {
            client,
            queries,
            fallback,
        }
    }

    pub async fn query(&self, op: &str, variables: Value, ttl: Duration) -> Result<Value> {
        let path = self.queries.path(op, &variables)?;
        let r = self
            .client
            .send(
                Request::get(Target::Pathfinder(path), JSON)
                    .ttl(ttl)
                    .cache_if(cacheable),
            )
            .await;
        let bytes = match r {
            Ok(b) => b,
            Err(Error::Http {
                status: 400 | 404 | 412,
                ..
            }) => return Err(Error::QueryRejected(op.to_owned())),
            Err(e) => return Err(e),
        };
        let v: Value = serde_json::from_slice(&bytes)
            .map_err(|_| Error::Protocol(format!("{op}: bad JSON")))?;
        check_errors(op, &v)?;
        Ok(v)
    }
}

pub fn search_variables(query: &str) -> Value {
    json!({
        "searchTerm": query,
        "offset": 0,
        "limit": 10,
        "numberOfTopResults": 5,
        "includeAudiobooks": false,
        "includePreReleases": false,
        "includeLocalConcertsField": false,
        "includeArtistHasConcertsField": false,
    })
}

pub fn home_variables(time_zone: &str) -> Value {
    json!({
        "timeZone": time_zone,
        "sp_t": "",
        "facet": "",
        "sectionItemsLimit": 10,
    })
}

fn items<'a>(v: &'a Value, ptr: &str) -> impl Iterator<Item = &'a Value> {
    v.pointer(ptr)
        .and_then(Value::as_array)
        .map(|a| a.iter())
        .into_iter()
        .flatten()
}

/// `data.searchV2` -> results.
pub fn parse_search(query: &str, v: &Value) -> Result<SearchResults> {
    let s = v
        .pointer("/data/searchV2")
        .ok_or_else(|| Error::QueryRejected(OP_SEARCH.into()))?;
    Ok(SearchResults {
        query: query.to_owned(),
        tracks: items(s, "/tracksV2/items")
            .filter_map(|i| i.pointer("/item/data"))
            .filter_map(types::gql_track)
            .collect(),
        artists: items(s, "/artists/items")
            .filter_map(|i| i.get("data"))
            .filter_map(types::gql_artist)
            .collect(),
        albums: items(s, "/albumsV2/items")
            .filter_map(|i| i.get("data"))
            .filter_map(types::gql_album)
            .collect(),
        playlists: items(s, "/playlists/items")
            .filter_map(|i| i.get("data"))
            .filter_map(types::gql_playlist)
            .collect(),
    })
}

/// `data.home.sectionContainer.sections` -> shelves of cards.
pub fn parse_home(v: &Value) -> Result<HomeFeed> {
    let sections = v
        .pointer("/data/home/sectionContainer/sections/items")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::QueryRejected(OP_HOME.into()))?;
    let shelves = sections
        .iter()
        .filter_map(|sec| {
            let title = sec
                .pointer("/data/title/transformedLabel")
                .or_else(|| sec.pointer("/data/title/text"))
                .and_then(Value::as_str)?
                .to_owned();
            let cards: Vec<Card> = items(sec, "/sectionItems/items")
                .filter_map(|it| {
                    let mut data = it.pointer("/content/data")?.clone();
                    if data.get("uri").is_none()
                        && let (Some(obj), Some(uri)) = (data.as_object_mut(), it.get("uri"))
                    {
                        obj.insert("uri".into(), uri.clone());
                    }
                    match data.get("__typename").and_then(Value::as_str) {
                        Some("Playlist") => types::gql_playlist(&data).map(Card::Playlist),
                        Some("Album") => types::gql_album(&data).map(Card::Album),
                        Some("Artist") => types::gql_artist(&data).map(Card::Artist),
                        _ => None, // episodes, shows, ads...
                    }
                })
                .collect();
            (!cards.is_empty()).then_some(Shelf {
                title,
                items: cards,
            })
        })
        .collect();
    Ok(HomeFeed { shelves })
}

/// IANA zone from `TZ`, else UTC (no tz database crate).
fn time_zone() -> String {
    std::env::var("TZ")
        .ok()
        .map(|t| t.trim_start_matches(':').to_owned())
        .filter(|t| t.contains('/'))
        .unwrap_or_else(|| "UTC".into())
}

impl SearchApi for Pathfinder {
    fn search<'a>(&'a self, query: &'a str) -> BoxFuture<'a, Result<SearchResults>> {
        Box::pin(async move {
            let r = self
                .query(OP_SEARCH, search_variables(query), TTL_SEARCH)
                .await
                .and_then(|v| parse_search(query, &v));
            match r {
                Err(e) if should_fall_back(&e) => {
                    tracing::warn!(error = %e, "pathfinder search failed; using spclient search");
                    self.fallback.searchview(query).await
                }
                other => other,
            }
        })
    }
}

impl HomeApi for Pathfinder {
    fn home(&self) -> BoxFuture<'_, Result<HomeFeed>> {
        Box::pin(async move {
            let r = self
                .query(OP_HOME, home_variables(&time_zone()), TTL_HOME)
                .await
                .and_then(|v| parse_home(&v));
            match r {
                Ok(h) if !h.shelves.is_empty() => Ok(h),
                Ok(_) => self.fallback.recent_home().await,
                Err(e) if should_fall_back(&e) => {
                    tracing::warn!(error = %e, "home query failed; building Home from recently played");
                    self.fallback.recent_home().await
                }
                Err(e) => Err(e),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_hashes_are_well_formed() {
        let q = Queries::default();
        for op in [OP_SEARCH, OP_HOME] {
            assert!(is_sha256(q.hash(op).unwrap()), "{op}");
        }
    }

    #[test]
    fn overrides_replace_valid_hashes_only() {
        let good = "a".repeat(64);
        let q = Queries::with_overrides(&format!(
            r#"{{"searchDesktop": "{good}", "home": "not-a-hash", "newOp": "{}"}}"#,
            "B".repeat(64)
        ));
        assert_eq!(q.hash(OP_SEARCH), Some(good.as_str()));
        assert_eq!(q.hash(OP_HOME), Queries::default().hash(OP_HOME));
        assert_eq!(q.hash("newOp"), Some("b".repeat(64).as_str()));
        assert_eq!(Queries::with_overrides("garbage"), Queries::default());
    }

    #[test]
    fn query_path_encodes_variables_and_hash() {
        let q = Queries::default();
        let p = q.path(OP_SEARCH, &search_variables("a b&c")).unwrap();
        let url = url::Url::parse(&format!("https://x{p}")).unwrap();
        let pairs: HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(url.path(), "/pathfinder/v1/query");
        assert_eq!(pairs["operationName"], "searchDesktop");
        let vars: Value = serde_json::from_str(&pairs["variables"]).unwrap();
        assert_eq!(vars["searchTerm"], "a b&c");
        let ext: Value = serde_json::from_str(&pairs["extensions"]).unwrap();
        assert_eq!(
            ext["persistedQuery"]["sha256Hash"],
            q.hash(OP_SEARCH).unwrap()
        );
        assert!(matches!(
            q.path("unknownOp", &json!({})),
            Err(Error::QueryRejected(_))
        ));
    }

    #[test]
    fn graphql_errors() {
        let rejected = json!({"errors": [{"message": "PersistedQueryNotFound"}]});
        assert!(matches!(
            check_errors("op", &rejected),
            Err(Error::QueryRejected(_))
        ));
        let partial = json!({"errors": [{"message": "x"}], "data": {"a": 1}});
        assert!(check_errors("op", &partial).is_ok());
        assert!(check_errors("op", &json!({"data": {}})).is_ok());
    }

    #[test]
    fn only_complete_answers_are_cached() {
        assert!(cacheable(br#"{"data": {"x": 1}}"#));
        assert!(cacheable(br#"{"data": {"x": 1}, "errors": []}"#));
        assert!(!cacheable(
            br#"{"data": {"x": 1}, "errors": [{"message": "m"}]}"#
        ));
        assert!(!cacheable(br#"{"data": null}"#));
        assert!(!cacheable(b"not json"));
    }

    #[test]
    fn fallback_policy() {
        assert!(should_fall_back(&Error::QueryRejected("x".into())));
        for status in [401, 403, 500, 503] {
            assert!(should_fall_back(&Error::Http {
                status,
                message: String::new()
            }));
        }
        assert!(should_fall_back(&Error::auth("token")));
        assert!(should_fall_back(&Error::Protocol("shape".into())));
        assert!(!should_fall_back(&Error::Net("offline".into())));
        assert!(!should_fall_back(&Error::RateLimited(5)));
    }

    #[test]
    fn home_sections_become_shelves() {
        let v = json!({"data": {"home": {"sectionContainer": {"sections": {"items": [
            {"data": {"title": {"transformedLabel": "Made for you"}},
             "sectionItems": {"items": [
                {"uri": "spotify:playlist:p1", "content": {"data": {"__typename": "Playlist", "name": "Daily Mix 1"}}},
                {"uri": "spotify:episode:e1", "content": {"data": {"__typename": "Episode"}}}
             ]}},
            {"data": {"title": {"transformedLabel": "Empty"}}, "sectionItems": {"items": []}}
        ]}}}}});
        let h = parse_home(&v).unwrap();
        assert_eq!(h.shelves.len(), 1);
        assert_eq!(h.shelves[0].title, "Made for you");
        assert!(matches!(&h.shelves[0].items[0], Card::Playlist(p) if p.id == "p1"));
        assert!(matches!(
            parse_home(&json!({"data": {}})),
            Err(Error::QueryRejected(_))
        ));
    }
}
