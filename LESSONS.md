# Lessons

Things learned while building Ferrotune. Add new entries at the top.

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
