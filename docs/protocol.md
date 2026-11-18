# Protocol notes (Ferrotune)

Ferrotune talks to three things: the Spotify **accounts** service (OAuth),
the Spotify **Web API** (metadata, library, search) and the Spotify
**access points** via librespot (audio). On Linux it also serves **MPRIS**
on the session bus.

## 1. Login: OAuth 2.0 Authorization Code + PKCE

Implemented in `src/auth/oauth.rs` (own implementation, ~300 lines, so
every piece is unit-tested; `librespot-oauth` is not used).

1. Generate a 64-char verifier from the RFC 7636 unreserved set and its
   S256 challenge (`BASE64URL(SHA256(verifier))`, no padding). Verified
   against the RFC 7636 Appendix B test vector.
2. Generate a random `state` (16 bytes, base64url).
3. Open `https://accounts.spotify.com/authorize?client_id=…&response_type=code&redirect_uri=http://127.0.0.1:8898/login&code_challenge_method=S256&code_challenge=…&state=…&scope=…`
   with `xdg-open` / `open` / `rundll32`; the URL can also be copied.
4. A tokio `TcpListener` on `127.0.0.1:<port>` accepts the browser redirect,
   checks `state`, handles `error=access_denied`, ignores `/favicon.ico`,
   answers with a small HTML page and returns the code (5 min timeout).
5. `POST /api/token` (form): `grant_type=authorization_code, code,
   redirect_uri, client_id, code_verifier` → `access_token`,
   `refresh_token`, `expires_in`.
6. Refresh: `grant_type=refresh_token, refresh_token, client_id`. If no new
   refresh token is returned the old one is kept. `invalid_grant` is treated
   as "session revoked" → keyring cleared, back to the login screen.

Scopes: `streaming user-read-private user-library-read user-library-modify
playlist-read-private playlist-read-collaborative user-follow-read
user-top-read user-read-recently-played`.

The client id comes from `FERROTUNE_CLIENT_ID` or `config.json`; users
register their own app (README). Spotify only accepts `127.0.0.1` loopback
redirect URIs.

### Keyring entries (service `dev.ferro.ferrotune`)

| Account key | Content |
|---|---|
| `oauth` | JSON `{access_token, refresh_token, expires_at, scope}` |
| `librespot` | JSON `{username, auth_data(base64)}` — librespot reusable credentials |

Nothing else is persisted; the SQLite cache contains no credentials.

## 2. Web API (`src/net/webapi.rs`)

Base `https://api.spotify.com/v1`, `Authorization: Bearer <token>`.

| Screen | Endpoints |
|---|---|
| Profile | `GET /me` |
| Library | `GET /me/playlists` (offset, 50), `GET /me/albums` (offset, 50), `GET /me/following?type=artist` (cursor `after`), `GET /me/tracks?limit=1` (count) |
| Liked Songs | `GET /me/tracks?limit=50&offset=` (infinite scroll) |
| Playlist | `GET /playlists/{id}` then `GET /playlists/{id}/tracks?limit=100&offset=` |
| Album | `GET /albums/{id}` (+ `/albums/{id}/tracks` for > 50 tracks) |
| Artist | `GET /artists/{id}`, `/artists/{id}/top-tracks?market=from_token`, `/artists/{id}/albums?include_groups=album,single` |
| Search | `GET /search?q=&type=track,artist,album,playlist&limit=10` |
| Home | `GET /me/player/recently-played`, `/me/top/artists`, `/me/top/tracks` |
| Like | `PUT`/`DELETE /me/tracks?ids=`, `GET /me/tracks/contains?ids=` |

Robustness rules (all covered by `tests/webapi.rs`):

- **Paging** is computed from `offset + raw entries` rather than the `next`
  URL, so it works against a mock server and is not confused by entries we
  drop (episodes, `null` tracks). Library lists are capped at 500.
- **Lenient parsing:** list entries are parsed individually; unknown or
  malformed entries are dropped. Playlist entries accept both `track` and
  the newer `item` field name.
- **401** → refresh once and retry; second 401 → revoked.
- **429** → `Retry-After` ≤ 5 s is slept inline (max 3 tries), otherwise
  `Error::RateLimited(secs)` is shown.
- **5xx** → one retry after 300 ms.
- Body-less `PUT`/`DELETE` send `Content-Length: 0` (Spotify answers 411
  otherwise).
- reqwest errors are stripped of their URL before they can be logged.

Spotify has restricted several endpoints for newly created apps over time
(recommendations, featured playlists, related artists, browse categories).
Ferrotune only uses endpoints that were available to development-mode apps
when it was written; Home is built from the user's own history and top
items, and Search's "Browse all" tiles are local genre shortcuts. Artist
top tracks degrade to an empty list if refused.

## 3. Audio: librespot (`src/net/playback.rs`)

- `librespot-core` `Session` connected with
  `Credentials::with_access_token(web_api_token)` on first login (the token
  carries the `streaming` scope), afterwards with the stored reusable
  credentials (`AUTHENTICATION_STORED_SPOTIFY_CREDENTIALS`). No librespot
  `Cache` is created, so librespot never writes credentials or audio to disk.
- `librespot-playback` `Player` with the `SoftMixer` (volume 0..1 →
  0..65535) and the **rodio** backend (ALSA on Linux, CoreAudio on macOS,
  WASAPI on Windows). TLS via rustls + webpki roots (no OpenSSL).
- Bitrate 96/160/320 from config; `position_update_interval = 1 s`.
- The engine runs as a tokio task fed by a command channel
  (`Load/Play/Pause/Seek/SetVolume/Stop/Attach`) and emits
  `PlaybackEvent`s that the app turns into state. Events for a URI other than
  the current track are ignored (stale end-of-track after a skip).
- The queue (order, shuffle, repeat, "Next in queue") lives in the app
  (`app/state.rs::Queue`); librespot only plays single tracks. Gapless
  preloading is not wired up yet.
- Audio session failures retry with exponential backoff (1 s → 60 s).
- `--mock` uses `MockEngine`, a simulated clock with the same events.

## 4. MPRIS (`src/net/mpris.rs`, Linux only)

Bus name `org.mpris.MediaPlayer2.ferrotune`, object `/org/mpris/MediaPlayer2`,
`DesktopEntry = dev.ferro.ferrotune` (matches the Wayland `app_id`).

- Implemented with `mpris-server` 0.9 (`Server` + our own `Send` struct
  implementing `RootInterface` / `PlayerInterface`).
- The app pushes a `MediaSnapshot` after every update; the server diffs it
  against the previous one and emits `PropertiesChanged` for
  `PlaybackStatus, Metadata, Volume, Shuffle, LoopStatus, CanGoNext,
  CanGoPrevious`, plus `Seeked` when the position jumps.
- Track ids: `/dev/ferro/ferrotune/track/<base62 id>`. `mpris:artUrl` is the
  300 px `i.scdn.co` image.
- Methods `Play, Pause, PlayPause, Stop, Next, Previous, Seek, SetPosition`
  and the writable `Volume, Shuffle, LoopStatus` map to UI messages;
  `Raise` focuses the window, `Quit` exits. `OpenUri` is not supported.
- Loop mapping: Repeat off ↔ `None`, context ↔ `Playlist`, track ↔ `Track`.
- `tests/mpris.rs` checks properties and method calls on a private bus
  (`dbus-run-session`).

## 5. UI reference

Values in `src/ui/theme.rs` (recreated, not copied from Spotify assets):

| Token | Value |
|---|---|
| Window background | `#000000` |
| Panels (sidebar/main/queue) | `#121212`, radius 8, 8 px gaps |
| Elevated / hover / selected | `#282828` / `#1f1f1f` / `#2a2a2a` |
| Text primary / secondary / muted | `#ffffff` / `#b3b3b3` / `#6a6a6a` |
| Accent (play buttons, active state) | `#1ed760` |
| Track row hover / selected | white 10 % / 30 % |
| Toast | `#2e77d0` |
| Font | Inter 400/600/700; hero title 72 px, section 24 px, rows 16/13 px |
| Sizes | sidebar 300 px, queue 320 px, top bar 64 px, player 72 px, rows 56 px, card art 156 px, header art 192 px |

Headers use a vertical gradient from the (brightness-normalised) average
colour of the cover art into the panel colour.
