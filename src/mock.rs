//! Deterministic mock data for tests and `--mock` screenshot mode.
//! All names are fictional; images are generated procedurally.

use crate::error::{Error, Result};
use crate::net::images::DecodedImage;
use crate::net::model::*;

const ARTISTS: &[(&str, &str, u64)] = &[
    ("ar01", "Nova Harbor", 2_481_903),
    ("ar02", "The Paper Lanterns", 812_334),
    ("ar03", "Mira Solenne", 5_120_877),
    ("ar04", "Coastal Static", 341_210),
    ("ar05", "Juniper Vale", 1_904_552),
    ("ar06", "Orbit & Ember", 664_018),
    ("ar07", "Delta Grey", 98_771),
    ("ar08", "Saffron Skies", 3_007_456),
];

/// (id, name, artist index, year, kind, track names)
/// (id, name, artist index, release date, kind, track names)
type AlbumRow = (
    &'static str,
    &'static str,
    usize,
    &'static str,
    AlbumKind,
    &'static [&'static str],
);

const ALBUMS: &[AlbumRow] = &[
    (
        "al01",
        "Midnight Cartography",
        0,
        "2024-03-08",
        AlbumKind::Album,
        &[
            "North of Nowhere",
            "Glass Satellites",
            "Tidewater",
            "Paper Moon Radio",
            "Lighthouse Keeper",
            "Static Bloom",
            "Harbor Lights",
            "Slow Orbit",
            "The Long Way Home",
            "Afterglow",
        ],
    ),
    (
        "al02",
        "Kites Over Kyoto",
        1,
        "2023-10-20",
        AlbumKind::Album,
        &[
            "First Light",
            "Lantern Festival",
            "Wind Chimes",
            "Origami Hearts",
            "Rain on Cedar",
            "Night Market",
            "Paper Boats",
            "Hanami",
        ],
    ),
    (
        "al03",
        "Velvet Hours",
        2,
        "2025-01-17",
        AlbumKind::Album,
        &[
            "Velvet Hours",
            "Golden Thread",
            "Run to the River",
            "Sugarglass",
            "Wildflower Season",
            "Only in Dreams",
            "Neon Prayer",
            "Honey & Smoke",
            "Silver Lining",
            "Stay",
            "Everblue",
            "Closing Time",
        ],
    ),
    (
        "al04",
        "Signal Lost",
        3,
        "2022-06-03",
        AlbumKind::Album,
        &[
            "Signal Lost",
            "Analog Heart",
            "Interference",
            "Ghost Frequency",
            "Cathode",
            "White Noise Lullaby",
        ],
    ),
    (
        "al05",
        "Evergreen",
        4,
        "2024-09-27",
        AlbumKind::Album,
        &[
            "Pinecone",
            "Evergreen",
            "Mountain Song",
            "Campfire Stories",
            "Northern Lights",
            "Snowmelt",
            "Trailhead",
            "Woodsmoke",
        ],
    ),
    (
        "al06",
        "Gravity Wells",
        5,
        "2025-05-02",
        AlbumKind::Single,
        &["Gravity Wells", "Event Horizon"],
    ),
    (
        "al07",
        "Monochrome",
        6,
        "2021-11-12",
        AlbumKind::Album,
        &[
            "Charcoal",
            "Silver Screen",
            "Graphite",
            "Ash & Ivory",
            "Shades",
            "Slate",
            "Fog",
        ],
    ),
    (
        "al08",
        "Desert Bloom",
        7,
        "2024-04-19",
        AlbumKind::Album,
        &[
            "Desert Bloom",
            "Mirage",
            "Saffron",
            "Dune Sea",
            "Oasis",
            "Sandglass",
            "Sirocco",
            "Starlit Caravan",
            "Cactus Flower",
        ],
    ),
];

const PLAYLISTS: &[(&str, &str, &str, &str)] = &[
    (
        "pl01",
        "Late Night Drive",
        "Synths, city lights and empty highways.",
        "Mock Listener",
    ),
    (
        "pl02",
        "Focus Flow",
        "Instrumental beats to keep you in the zone.",
        "Ferrotune",
    ),
    (
        "pl03",
        "Sunday Morning",
        "Slow coffee, soft vocals.",
        "Mock Listener",
    ),
    (
        "pl04",
        "Indie Garden",
        "Fresh indie picks, updated weekly.",
        "Ferrotune",
    ),
    (
        "pl05",
        "Workout Mix",
        "High tempo, no skips.",
        "Mock Listener",
    ),
    (
        "pl06",
        "Rainy Day Jazz",
        "Brushes, upright bass and rain on the window.",
        "Ferrotune",
    ),
    ("pl07", "Road Trip 2025", "Windows down.", "Mock Listener"),
    (
        "pl08",
        "Deep Focus",
        "Ambient textures for deep work.",
        "Ferrotune",
    ),
    (
        "pl09",
        "Throwback Jams",
        "The songs you forgot you loved.",
        "Mock Listener",
    ),
    ("pl10", "Chill Vibes", "Kick back and relax.", "Ferrotune"),
];

pub fn image_url(seed: &str) -> String {
    format!("mock://{seed}")
}

/// Two variants like the real API (which returns 64/300/640 px).
fn images(seed: &str) -> Vec<Image> {
    vec![
        Image {
            url: image_url(seed),
            width: Some(300),
            height: Some(300),
        },
        Image {
            url: format!("{}?64", image_url(seed)),
            width: Some(64),
            height: Some(64),
        },
    ]
}

fn artist(i: usize) -> Artist {
    let (id, name, followers) = ARTISTS[i];
    Artist {
        id: id.into(),
        uri: format!("spotify:artist:{id}"),
        name: name.into(),
        images: images(id),
        followers: Some(followers),
        genres: vec!["indie".into(), "dream pop".into()],
    }
}

fn track_duration(album: usize, n: usize) -> u32 {
    150_000 + ((album * 7919 + n * 104_729) % 140_000) as u32
}

fn album_full(i: usize) -> Album {
    let (id, name, ar, date, kind, names) = ALBUMS[i];
    let a = artist(ar);
    let aref = AlbumRef {
        id: id.into(),
        name: name.into(),
        images: images(id),
    };
    let tracks = names
        .iter()
        .enumerate()
        .map(|(n, t)| Track {
            id: format!("{id}t{n:02}"),
            uri: format!("spotify:track:{id}t{n:02}"),
            name: (*t).into(),
            artists: if n % 4 == 3 {
                vec![
                    ArtistRef {
                        id: a.id.clone(),
                        name: a.name.clone(),
                    },
                    ArtistRef {
                        id: ARTISTS[(ar + 3) % ARTISTS.len()].0.into(),
                        name: ARTISTS[(ar + 3) % ARTISTS.len()].1.into(),
                    },
                ]
            } else {
                vec![ArtistRef {
                    id: a.id.clone(),
                    name: a.name.clone(),
                }]
            },
            album: Some(aref.clone()),
            duration_ms: track_duration(i, n),
            explicit: n % 5 == 2,
            playable: true,
            added_at: Some(format!("2025-0{}-1{}T10:00:00Z", 1 + (n % 9), n % 10)),
        })
        .collect::<Vec<_>>();
    Album {
        id: id.into(),
        uri: format!("spotify:album:{id}"),
        name: name.into(),
        artists: vec![ArtistRef {
            id: a.id,
            name: a.name,
        }],
        images: images(id),
        release_date: date.into(),
        kind,
        total_tracks: tracks.len() as u32,
        tracks,
    }
}

fn all_tracks() -> Vec<Track> {
    (0..ALBUMS.len())
        .flat_map(|i| album_full(i).tracks)
        .collect()
}

fn playlist_tracks(i: usize) -> Vec<Track> {
    let all = all_tracks();
    let n = 14 + (i * 5) % 20;
    (0..n)
        .map(|k| all[(i * 11 + k * 7) % all.len()].clone())
        .collect()
}

fn playlist_at(i: usize) -> Playlist {
    let (id, name, desc, owner) = PLAYLISTS[i];
    Playlist {
        id: id.into(),
        uri: format!("spotify:playlist:{id}"),
        name: name.into(),
        description: desc.into(),
        owner: owner.into(),
        images: images(id),
        total_tracks: playlist_tracks(i).len() as u32,
    }
}

pub fn user() -> User {
    User {
        id: "mockuser".into(),
        display_name: "Mock Listener".into(),
        image: None,
        product: Some("premium".into()),
    }
}

pub fn library() -> Library {
    Library {
        playlists: (0..PLAYLISTS.len()).map(playlist_at).collect(),
        albums: (0..ALBUMS.len())
            .map(|i| {
                let mut a = album_full(i);
                a.tracks.clear();
                a
            })
            .collect(),
        artists: (0..ARTISTS.len()).map(artist).collect(),
        liked_total: liked_tracks().len() as u32,
    }
}

fn liked_tracks() -> Vec<Track> {
    all_tracks().into_iter().step_by(2).collect()
}

pub fn is_liked(id: &str) -> bool {
    liked_tracks().iter().any(|t| t.id == id)
}

pub fn liked(offset: u32) -> Page<Track> {
    let all = liked_tracks();
    let total = all.len() as u32;
    Page::new(
        all.into_iter().skip(offset as usize).take(50).collect(),
        offset,
        total,
    )
}

pub fn home() -> HomeFeed {
    let all = all_tracks();
    HomeFeed {
        recent_tracks: [0, 12, 25, 33, 40, 50, 58, 63]
            .iter()
            .map(|&i| all[i % all.len()].clone())
            .collect(),
        top_artists: [2, 0, 7, 4, 1, 5].iter().map(|&i| artist(i)).collect(),
        top_tracks: [24, 2, 61, 37]
            .iter()
            .map(|&i| all[i % all.len()].clone())
            .collect(),
    }
}

pub fn playlist(id: &str) -> Result<(Playlist, Page<Track>)> {
    let i = PLAYLISTS
        .iter()
        .position(|p| p.0 == id)
        .ok_or_else(|| Error::Http {
            status: 404,
            message: "not found".into(),
        })?;
    let tracks = playlist_tracks(i);
    let total = tracks.len() as u32;
    Ok((playlist_at(i), Page::new(tracks, 0, total)))
}

pub fn album(id: &str) -> Result<Album> {
    ALBUMS
        .iter()
        .position(|a| a.0 == id)
        .map(album_full)
        .ok_or_else(|| Error::Http {
            status: 404,
            message: "not found".into(),
        })
}

pub fn artist_page(id: &str) -> Result<ArtistPage> {
    let i = ARTISTS
        .iter()
        .position(|a| a.0 == id)
        .ok_or_else(|| Error::Http {
            status: 404,
            message: "not found".into(),
        })?;
    let mut albums: Vec<Album> = (0..ALBUMS.len())
        .filter(|&k| ALBUMS[k].2 == i || (k + i) % 3 == 0)
        .map(album_full)
        .collect();
    // The artist's own releases first, like the real discography.
    albums.sort_by_key(|a| a.artists.first().is_none_or(|r| r.id != id));
    let mut top: Vec<Track> = albums
        .iter()
        .flat_map(|a| a.tracks.clone())
        .take(10)
        .collect();
    if top.is_empty() {
        top = all_tracks().into_iter().take(5).collect();
    }
    Ok(ArtistPage {
        artist: artist(i),
        top_tracks: top,
        albums: albums
            .into_iter()
            .map(|mut a| {
                a.tracks.clear();
                a
            })
            .collect(),
    })
}

pub fn search(query: &str) -> SearchResults {
    let q = query.to_lowercase();
    let m = |s: &str| s.to_lowercase().contains(&q);
    let mut tracks: Vec<Track> = all_tracks()
        .into_iter()
        .filter(|t| m(&t.name) || t.artists.iter().any(|a| m(&a.name)))
        .take(10)
        .collect();
    let artists: Vec<Artist> = (0..ARTISTS.len())
        .map(artist)
        .filter(|a| m(&a.name))
        .collect();
    let albums: Vec<Album> = library()
        .albums
        .into_iter()
        .filter(|a| m(&a.name) || m(&a.artist_line()))
        .collect();
    let playlists: Vec<Playlist> = (0..PLAYLISTS.len())
        .map(playlist_at)
        .filter(|p| m(&p.name) || m(&p.description))
        .collect();
    if tracks.is_empty() && !artists.is_empty() {
        tracks = artist_page(&artists[0].id)
            .map(|p| p.top_tracks)
            .unwrap_or_default();
    }
    SearchResults {
        query: query.into(),
        tracks,
        artists,
        albums,
        playlists,
    }
}

fn hash(s: &str) -> u64 {
    // FNV-1a: stable across runs and platforms.
    s.bytes().fold(0xcbf29ce484222325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
    })
}

fn hsv(h: f32, s: f32, v: f32) -> [u8; 3] {
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match (h as u32) / 60 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [
        ((r + m) * 255.0) as u8,
        ((g + m) * 255.0) as u8,
        ((b + m) * 255.0) as u8,
    ]
}

/// Procedural "album art": a diagonal two-colour gradient with soft rings.
pub fn image(url: &str, max_px: u32) -> DecodedImage {
    let size = max_px.clamp(16, 300);
    let h = hash(url.split('?').next().unwrap_or(url));
    let hue1 = (h % 360) as f32;
    let hue2 = (hue1 + 40.0 + ((h >> 12) % 80) as f32) % 360.0;
    let c1 = hsv(hue1, 0.65, 0.85);
    let c2 = hsv(hue2, 0.75, 0.35);
    let cx = (0.3 + ((h >> 20) % 40) as f32 / 100.0) * size as f32;
    let cy = (0.3 + ((h >> 28) % 40) as f32 / 100.0) * size as f32;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let t = (x + y) as f32 / (2.0 * size as f32);
            let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt() / size as f32;
            let ring = if ((d * 14.0) as u32).is_multiple_of(2) {
                1.0
            } else {
                0.9
            };
            for k in 0..3 {
                let v = f32::from(c1[k]) * (1.0 - t) + f32::from(c2[k]) * t;
                rgba.push((v * ring).min(255.0) as u8);
            }
            rgba.push(255);
        }
    }
    DecodedImage::from_rgba(size, size, rgba)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_data_is_consistent() {
        let lib = library();
        assert_eq!(lib.playlists.len(), PLAYLISTS.len());
        for p in &lib.playlists {
            let (pl, page) = playlist(&p.id).unwrap();
            assert_eq!(pl.total_tracks, page.total);
        }
        assert!(album("nope").is_err());
        assert!(!search("velvet").tracks.is_empty());
        assert_eq!(liked(0).items.len() as u32, liked(0).total.min(50));
        let img = image("mock://al01", 64);
        assert_eq!(img.byte_size(), 64 * 64 * 4);
        assert_eq!(image("mock://al01", 64).rgba, img.rgba, "deterministic");
    }
}
