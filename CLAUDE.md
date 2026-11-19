# Ferrotune

## Overview
Native Rust Spotify desktop client (unofficial, Premium only). iced 0.14 UI
recreating the official dark look. One librespot 0.8 session (desktop client
id, zero-config browser login) drives audio, Spotify Connect and the internal
spclient/pathfinder APIs; no public Web API. MPRIS on Linux; SQLite cache.

## Commands
- `cargo build --release` / `cargo run -- --mock`
- `cargo fmt --check && cargo clippy --all-targets -- -D warnings`
- `dbus-run-session -- cargo test` (MPRIS integration test needs a bus)
- Screenshots: `ICED_BACKEND=tiny-skia xvfb-run -a -s "-screen 0 1280x800x24" ./target/debug/ferrotune --screenshot <screen> docs/screenshots/<screen>.png`
- Build jobs on shared machines: `CARGO_BUILD_JOBS=2`

## Architecture
See `docs/architecture.md` (shared Ferro* skeleton) and `docs/protocol.md`
(login, endpoints, persisted queries, Connect, MPRIS, UI reference).
- `src/app/` state + update (queue/nav reducers in `state.rs`)
- `src/ui/` views + `theme.rs` (only place with colours/sizes)
- `src/net/` session, internal-API backends (traits in `backend.rs`), playback, MPRIS
- `src/store/` SQLite cache, `src/auth/` browser login (PKCE) + keyring

## Conventions
- English everywhere; Conventional Commits; small commits.
- No `unwrap`/`expect` outside tests (crate-level deny).
- Only `auth/secrets.rs` touches `keyring`. Secrets use `Secret` (redacted Debug).
- `net`/`store`/`auth` never import iced widgets.
- New crates need a one-line justification in the PR.
- Views take `&App`; all mutation goes through `App::update`.

## Workflow quirks
- Work on feature branches into `development`; never push `main`/`development`.
- `version.yml` bumps the version in `Cargo.toml`; don't bump by hand.
- Tests never hit the network: use `wiremock` + `tests/fixtures/`.

## Deployment
No packaging yet. Release binary from `cargo build --release`;
`ferrotune --install-desktop-entry` writes the .desktop file + icon.

## Gotchas
- iced: keep default features on (x11/wayland). `Subscription::map` closures
  must be non-capturing.
- `tokio::time::sleep` must be created inside an `async` block for
  `Task::perform` (no runtime in unit tests otherwise).
- Spotify requires `http://127.0.0.1:<port>` redirect URIs (not localhost).
- Pathfinder persisted-query hashes rotate: they live only in
  `net/pathfinder.rs`; refresh steps in docs/protocol.md.
- Paging uses raw entry counts (`Page::raw_len`), since episodes/null items
  are dropped during conversion.
- librespot rodio backend needs ALSA dev libs on Linux.
- Right-click needs `ui/context_area.rs` (`mouse_area` has no window position).
  `update_ui` closes the context menu on every `Ui` except the exempt list at
  its top: add new passive events (hover, scroll, resize) there.
- Playlist edits are optimistic: keep the `EditSnapshot` rollback path (tested
  in `app/playlists.rs`) when adding new edit kinds.
- librespot's `Unavailable` has no reason: `net/audio_diag.rs` reads it from
  librespot's log. Audio key code 1 = Spotify denies local playback for the
  account (librespot #1649); play through Connect, never bypass it.
