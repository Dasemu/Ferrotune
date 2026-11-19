# Lessons

Things learned while building Ferrotune. Add new entries at the top.

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
