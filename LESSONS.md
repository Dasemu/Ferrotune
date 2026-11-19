# Lessons

Things learned while building Ferrotune. Add new entries at the top.

- **librespot hides why a track failed.** `PlayerEvent::Unavailable` has no
  reason; the cause is only in librespot's log. A tracing layer that reads
  librespot's own records (`net/audio_diag.rs`) turned "every song is
  unavailable" into "audio key refused with code 1" (librespot #1649), which
  no client-side retry fixes. Never auto-skip on a failure that hits every
  track: cap the streak and show one banner.
- **Owners are usernames, ownership is a capability.** Rootlist owners are
  raw usernames and the profile fetch is optional; compare with
  `session.username()` and prefer `capabilities.can_edit_items`, which also
  covers collaborative lists.

- **`mouse_area` reports no window position.** Its `on_move` point is local to
  the widget and its right-press carries nothing, so a context menu needs a
  tiny wrapper widget (`ui/context_area.rs`) that reads `cursor.position()`.
- **A release only reaches the widget under the cursor.** Drag-and-drop
  reordering therefore puts `on_release` on every row (the drop target) and a
  global mouse-up subscription, active only while dragging, to cancel strays.
- **Scroll events fire at layout time.** A "close on any action" rule must
  exempt `Scrolled`, and an open menu's backdrop swallows the wheel instead.
- **Edit by revision, not by guess.** playlist4 changes need the list's
  current revision; fetching it uncached right before each write avoids
  stale-base conflicts, and cached reads must be dropped after the write.

- **Sync releases back with a real merge.** A separate `chore(release)` commit
  on development edits the same version lines as the bump on main, so every
  later development→main PR conflicts. Merging main into development
  (`-X theirs`, then version = max) makes the release commit an ancestor.
- **A server-controlled duration is untrusted input.** `Instant + Duration`
  panics on overflow: clamp `Retry-After` and use `checked_add`.
- **`futures::Shared` alone is not a request coalescer.** If every waiter is
  dropped, the shared future stops being polled while holding its permit.
  Run the request in `tokio::spawn` and share the join handle.
- **librespot reports every AP login refusal as `PermissionDenied`.** Only
  "Bad credentials" may delete stored credentials.

- **One identity beats two.** v0.2 needed a developer-dashboard app for the
  Web API on top of librespot's session. Users rejected that. Everything now
  goes through the librespot session (desktop client id, login5 tokens,
  spclient/pathfinder), so there is one login, one token source and nothing
  to configure.
- **Persisted-query hashes are a moving target.** Keep them in one module,
  allow overriding them from a file, map "PersistedQueryNotFound" to a typed
  error, and always keep an spclient fallback so the screen still works.
- **`Spirc::new` connects the session itself.** Create the session
  unconnected and let Spirc connect it. If Connect fails for any reason other
  than bad credentials, start over with a plain session.
- **librespot-protocol does not compile every .proto.** collection2v2 is
  missing; a small hand-written codec with wire-level tests is cheaper than a
  protobuf code generator.
- **Cooldown after long 429s.** Sleeping through a 2-minute `Retry-After`
  inside a request freezes the UI. Fail fast for the remaining time instead,
  and serve stale cache where there is one.

- **Dead librespot sessions are silent.** After suspend/resume or a network
  drop every `load` came back `Unavailable` and the app skipped the whole
  queue. The engine now polls `Session::is_invalid()` / `Player::is_invalid()`
  and emits `SessionLost`; the app pauses, reconnects with backoff and
  reloads the track on play.
- **Refresh after 401 must be single-flight.** Parallel requests that all
  got a 401 each refreshed (and may rotate the refresh token). Compare with
  the token the request used before refreshing.
- **Loopback login listener:** bind before opening the browser, and ignore
  (400) anything without our `state` instead of aborting.
- **Release workflow:** detect dev→main via the commit's associated PRs
  (works for squash/rebase), guard on the bot actor rather than the commit
  message, and sync the MINOR bump back to development.

- **Paging must count raw entries.** Playlists contain episodes, removed
  tracks (`"track": null`) and local files. Filtering them before computing
  the next offset re-fetched overlapping pages; `Page::raw_len` fixes it.
  Found by the wiremock fixture test.
- **Parse list entries leniently.** One malformed entry (e.g. a local file
  with `"id": null` on its album) used to fail the whole page. `Paging`
  items are now parsed one by one and bad entries dropped.
- **`tokio::time::sleep()` needs a runtime at construction.** Wrap it in
  `async move { .. }` when handing it to `Task::perform`, or unit tests that
  call `update()` without a runtime panic.
- **mpris-server's ready-made `Player` is `!Send`.** Implementing
  `RootInterface`/`PlayerInterface` on our own `Send` struct with `Server`
  keeps it on the tokio runtime and makes the property mapping testable.
- **Keep mpris-server on 0.9** so it shares zbus 5 with iced/keyring (0.11
  pulls zbus 6, duplicating the D-Bus stack).
- **librespot without `Cache`** never writes credentials to disk; we read
  `session.auth_data()` after connect and store it in the keyring ourselves.
- **Screenshots under Xvfb** work with `ICED_BACKEND=tiny-skia`; give the
  async image loads ~2.5 s before calling `window::screenshot`.
- Bot pushes made with `GITHUB_TOKEN` never trigger `push` workflows, so the release workflow chains off the version workflow with `workflow_run` (conclusion `success`, branch `main`, checkout `ref: main` for the bump commit).
