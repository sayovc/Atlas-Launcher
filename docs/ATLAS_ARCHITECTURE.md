# Atlas Launcher — Architecture

Atlas is an original, independently written Rust desktop launcher for
Minecraft: Java Edition. This document covers what we learned from public
material about Horizon Launcher (a modern Rust launcher in the same product
category) and how Atlas implements each system on its own.

> Atlas is not affiliated with Horizon, Mojang or Microsoft. It contains no
> Horizon source code, branding, artwork or UI, and ships no Mojang-owned
> assets or code.

---

## 1. Research summary (public documentation only)

Sources: the Horizon project's public README, ROADMAP and
`docs/native-client.md` on GitHub, plus public news coverage. We did not read
or reuse Horizon source code.

| Topic | What Horizon publicly describes |
|---|---|
| Product | A Rust re-implementation of a Minecraft Java Edition release, distributed through a separate launcher. |
| Game files | Ships no Mojang data. A *prepare* step checks the user's own installed client/server jars against pinned checksums, runs Mojang's own exporters (needs a JDK), and extracts assets into a private per-version folder. A stamp file marks finished output, a lock file prevents concurrent preparation, and there is a "check only" mode plus JSON progress output. |
| Storage | Prepared resources live in a private per-version folder. Saves and options live in a separate game home (env override available). Logs go to `logs/latest.log` with date archives; crash reports go to `crash-reports/`. |
| Launcher | Installs/launches versions, has Microsoft sign-in (for online-mode servers), a Modrinth-backed mod browser, server hosting, and Java instances (vanilla + modded) on its roadmap. |
| Updates | Release checksums are signed (Ed25519) and the launcher refuses unsigned files. |
| CLI / agents | A control CLI (launch, state, logs, scripting) and MCP servers, plus a global "No AI" switch. |

**Key takeaway for Atlas.** Horizon's launcher manages builds of its *own
native game runtime* and feeds them data prepared from the user's legitimate
install. Atlas has no game runtime yet. So Atlas defines a **runtime
abstraction**:

* `vanilla-java`: launches the official Java client, with files obtained from
  Mojang's official, checksummed distribution endpoints. This is the first
  real target.
* `atlas-native` (future): a native Rust runtime. It needs the same
  *prepare-from-the-user's-own-install* pipeline and a signed Atlas build
  channel. The architecture reserves space for it, but it is not claimed to
  exist.

---

## 2. Workspace layout

```
atlas/
├── Cargo.toml              workspace (edition 2024, unsafe_code = forbid)
├── crates/
│   ├── atlas-shared/       AtlasError + ErrorCode, redaction, fs safety      [Phase 1 ✔]
│   ├── atlas-config/       platform paths, locator, typed Settings (TOML)    [Phase 1 ✔]
│   ├── atlas-logging/      tracing setup, JSON files, ring buffer, reader    [Phase 1 ✔]
│   ├── atlas-core/         the backend facade used by every front-end        [Phase 1 ✔]
│   ├── atlas-ui/           egui/eframe desktop app  → binary `atlas-launcher` [Phase 1 ✔]
│   ├── atlas-cli/          clap CLI                 → binary `atlas`          [Phase 1 ✔]
│   ├── atlas-downloads/    resumable, verified HTTP downloads                [Phase 2 ✔]
│   ├── atlas-versions/     version manifests, install/verify/repair, detect  [Phase 2 ✔]
│   ├── atlas-minecraft/    Java data layout, detection, preparation          [Phase 2/3]
│   ├── atlas-auth/         Microsoft → Xbox → Minecraft auth, keyring        [Phase 3]
│   ├── atlas-instances/    installations                                     [Phase 3]
│   ├── atlas-launch/       process supervision, game logs, crash detection   [Phase 3]
│   ├── atlas-servers/      server list, ping                                 [Phase 4]
│   ├── atlas-content/      mods, resource packs, modpacks, worlds            [Phase 4]
│   ├── atlas-mcp/          opt-in local MCP server                           [Phase 5]
│   └── atlas-updater/      signed self-update                                [Phase 5]
└── docs/
```

Differences from the suggested layout, and why:

* Crates live under `crates/` with an `atlas-` prefix. This keeps the root
  clean and avoids name clashes on crates.io.
* `backend/` is called **`atlas-core`**. It is a library, not a process. The
  GUI, CLI and MCP server each embed it, so no IPC is needed for a
  single-user desktop app.
* `ui/` and `launcher/` are merged into **`atlas-ui`**, the binary that *is*
  the launcher window.
* "mods" is called **`atlas-content`** because it also covers resource packs,
  shader packs, modpacks and worlds.
* Integration tests live next to the crate they exercise
  (`crates/*/tests/`). Cargo runs them with `cargo test --workspace`.

Only crates marked ✔ exist today. The others are created when their phase
starts; there are no empty stubs.

### Dependency direction

```
atlas-ui ─┐
atlas-cli ├─► atlas-core ─► {versions, instances, auth, launch, …} ─► atlas-config ─► atlas-shared
atlas-mcp ┘                                                         └► atlas-logging ─┘
```

Front-ends never call domain crates directly. They call `atlas_core::Atlas`,
so behaviour, validation and confirmation rules are identical everywhere.

### UI framework

The UI uses **egui/eframe 0.37 (glow backend)** rather than GPUI:

* GPUI's crates.io API is still moving fast and its Linux backend needs Vulkan.
* eframe is pure Rust, cross-platform, quick to compile, and renders under
  Xvfb + Mesa, so the UI can be screenshot-tested in CI.
* The immediate-mode model fits a UI that "just renders backend state". Each
  frame reads `Atlas::status()`, so the UI can't cache a stale or invented
  state.

The UI is isolated in one crate, so swapping frameworks later doesn't touch
the backend.

---

## 3. Data directories

Atlas resolves directories with the `directories` crate. Nothing is
hardcoded:

| Platform | Bootstrap (locator) | Default data root |
|---|---|---|
| Linux | `~/.config/atlaslauncher` | `~/.local/share/atlaslauncher` |
| Windows | `%APPDATA%\AtlasLauncher` | `%APPDATA%\AtlasLauncher` |
| macOS | `~/Library/Application Support/AtlasLauncher` | same |

* **Bootstrap dir** holds only `locator.toml` (`schema`, `data_root`). This
  file is what makes the data root relocatable.
* **Data root** holds everything else:

```
<data_root>/
├── settings.toml
├── accounts/         account metadata (ids, names, uuids) — never tokens
├── installations/<id>/installation.toml, game/ (saves, options, mods…)
├── versions/         installed version metadata + client jars
├── runtimes/         managed Java runtimes
├── cache/downloads/  content-addressed verified downloads (by hash)
└── logs/launcher/    atlas.log.YYYY-MM-DD (JSON lines)
```

Overrides: `ATLAS_CONFIG_DIR` and `ATLAS_DATA_DIR` (portable mode, tests).

**Atlas never writes into another launcher's folder.** `.minecraft`, Prism
instances and Horizon folders are only ever *read* (Phase 2/3 detection and
preparation).

### Moving the data root (implemented)

`Atlas::move_data_root(target, Confirmation)`:

1. Requires confirmation.
2. Target must be absolute, outside the current root (and not containing it),
   not a symlink, and missing or empty. Atlas never merges into an existing
   folder, so another launcher's data can't be overwritten.
3. Copies only Atlas-managed items, **without following symlinks**. A link
   to the user's real `.minecraft/saves` is skipped and reported, never
   duplicated or deleted.
4. Verifies file counts and byte totals per item. On any mismatch it deletes
   the partial copy and leaves the original untouched.
5. Points the locator at the verified copy, then removes the originals.
   Removal failures (for example, a log file open on Windows) are reported
   as leftovers, never treated as fatal.
6. Leaves unmanaged files in the old root alone.

---

## 4. Configuration

`settings.toml` maps to the strongly typed `atlas_config::Settings` struct,
with sections `general`, `appearance`, `downloads`, `game`, `memory`,
`installations`, `accounts`, `privacy` and `advanced` (including
`advanced.ai`).

* `schema = 1`. Files from a newer schema are rejected with a clear error.
* Every field has a default, so old or partial files load.
* `Settings::validate()` enforces invariants (memory bounds, min ≤ max,
  concurrency 1–32, UI scale, window size, log retention).
* An unparsable or invalid file is **backed up**
  (`settings.toml.invalid-<unix>`), replaced with defaults, and reported as a
  warning in both GUI and CLI. It is never silently overwritten.
* Unknown keys load fine but produce a warning.
* Writes are atomic: temp file, fsync, rename.
* Dotted-key access (`memory.max_mb`) powers `atlas settings get/set`, and
  later MCP `atlas_settings`.
* Checksum verification is deliberately **not** a setting.

---

## 5. Errors

`atlas_shared::AtlasError { code, message, details, suggestion }`:

* `code` is a stable `ErrorCode` serialised as `SCREAMING_SNAKE_CASE`, e.g.
  `AUTH_EXPIRED`, `VERSION_NOT_FOUND`, `DOWNLOAD_FAILED`, `CHECKSUM_MISMATCH`,
  `PREPARATION_FAILED`, `GAME_CRASHED`, `INVALID_INSTALLATION`,
  `CONFIRMATION_REQUIRED`, `PATH_TRAVERSAL` and `DATA_DIR_INVALID`.
* `details` always passes through redaction.
* The GUI shows the code, message, suggestion, and collapsible details with a
  Copy button. The CLI prints them, or with `--json` emits
  `{"ok":false,"error":{…}}`. Exit code 2 means invalid argument, 3 means
  confirmation required, and 1 is any other failure.

---

## 6. Logging

* Built on `tracing` + `tracing-subscriber`.
* **Files:** JSON lines, daily rotation, retention from
  `advanced.keep_log_files`, written by a non-blocking worker.
* **Redaction:** every file and stderr line passes through `RedactingWriter`,
  and every in-memory record is redacted too. Patterns cover OAuth fields,
  `Authorization: Bearer|XBL3.0`, JWTs, `--accessToken <x>` and Microsoft
  refresh tokens. Unit tests pin this behaviour.
* **Live viewer:** a `BufferLayer` feeds a bounded ring buffer (10k records).
  The GUI Logs page filters by level, source and text, follows the tail, and
  copies a single line or all visible lines.
* **Cross-process:** `atlas logs` parses the JSON files, so the CLI can see
  GUI activity.
* **Sources:** `launcher`, `game`, `crash`, `install`, `download`. An event
  sets one with the `log_source = "…"` field. Game stdout/stderr (Phase 3)
  is ingested under `game`, and crash reports under `crash`.

---

## 7. Versions and downloads (implemented in Phase 2)

**As built:**

* `atlas-downloads` uses `reqwest` with rustls and the platform verifier, so
  it honours system and corporate certificate stores.
  * Data streams into `<dest>.part` while being hashed, and the size is
    guarded so a server can't send more than announced.
  * The file is fsynced, verified (SHA-1 or SHA-256, plus size), then
    atomically renamed into place.
  * Resume uses `Range` and falls back to a clean restart when the server
    ignores it.
  * Retries use exponential backoff. A shared token bucket enforces
    `downloads.bandwidth_limit_kib`, and every await point honours a
    `CancellationToken`.
  * Valid files are never fetched twice. Candidate files from other
    launchers are reused only on a hash match, and they are copied, never
    moved.
* `atlas-versions`:
  * Fetches Mojang's manifest over HTTPS and caches it for offline use (an
    offline result is marked `stale`).
  * Checks each version JSON against the manifest's SHA-1, then every jar,
    library, native, logging config, asset index and asset object against
    the SHA-1 published one link up.
  * Evaluates library rules for the current OS/arch, ignores feature-gated
    rules, and resolves legacy `natives` classifiers.
  * Every metadata path goes through `safe_join`, and ids and hashes are
    validated.
  * An install works in `versions/.staging/<id>/` (resumable). Shared
    libraries and assets go straight to verified, shared locations.
    `install.toml` (every file with path, SHA-1, size and URL) is written
    last, then staging is renamed to `versions/<id>/`. A version counts as
    installed only when `install.toml` exists.
  * Verify re-hashes every recorded file. Repair re-downloads only the bad
    files, using the record's URLs. Remove deletes `versions/<id>/` only,
    and shared files are kept.
  * The update check flags a newer latest release than any installed
    release, using the cached list (no network).
  * Detection is read-only and covers the official launcher (including
    Flatpak), Prism (including Flatpak), MultiMC and `game.minecraft_dir`.
* `atlas-core` runs installs and repairs as background jobs on a Tokio
  runtime. It tracks lock-free progress (files, bytes, downloaded / reused /
  already valid) and supports cancellation. It refuses concurrent jobs on
  the same version, removing a busy version, and moving the data root while
  jobs run.
* `ATLAS_VERSION_MANIFEST_URL` and `ATLAS_ASSETS_BASE_URL` override the
  endpoints, for mirrors and tests.
* Atlas build channel (signed Ed25519 manifest): still future work.

**Verified against the real service:** `atlas install latest` downloaded
Minecraft 26.3 (5,217 files, 584 MB) in about 64 s. `atlas verify 26.3`
re-hashed all 5,219 recorded files in 4.4 s.

### Original design notes

* **Catalog sources** implement a `VersionSource` trait:
  * `MojangSource` reads `piston-meta.mojang.com/mc/game/version_manifest_v2.json`.
    Each version JSON carries a SHA-1 for itself, and each library, asset
    index, asset object and client jar carries a SHA-1 and size.
  * `AtlasBuildSource` (future) reads a release manifest that is **signed
    with Ed25519**. The public key is compiled into Atlas, and anything
    unsigned is refused.
* **Downloads** (`atlas-downloads`):
  * Built on `reqwest` with rustls.
  * Writes to `<file>.part`, resumes with `Range` when the server supports
    it, and streams into a hasher.
  * After completion it compares the expected hash and size, then does an
    atomic rename into the content-addressed cache
    (`cache/downloads/<algo>/<hh>/<hash>`).
  * A mismatch means `CHECKSUM_MISMATCH`, the `.part` file is deleted, and
    the download is retried with bounded backoff.
  * Concurrency comes from `downloads.max_concurrent`, with an optional rate
    limit. Every job has a cancellation token.
* **Installation of a version** is transactional:
  * Resolve the full file list.
  * Fetch or verify everything into the cache.
  * Materialise into a `versions/<id>.staging/` directory.
  * Write `version.toml` with the hashes of every file.
  * Rename the staging directory to `versions/<id>/`. An interrupted install
    leaves only staging junk, which is cleaned on the next start, never a
    half-valid version.
* **Verify/repair** re-hashes against `version.toml` and re-fetches only bad
  or missing files.

## 8. Minecraft preparation (Phase 2/3 design)

* **Detection** probes, read-only:
  * The official launcher folders (`%APPDATA%\.minecraft`,
    `~/.minecraft`, `~/Library/Application Support/minecraft`).
  * Prism/MultiMC-style roots (`instances/*/instance.cfg`).
  * A user-selected folder (`game.minecraft_dir`).
* **Reuse:** if a detected install already has an asset object or library
  with the expected SHA-1, Atlas *copies* it into its cache after verifying
  it. Atlas never modifies or hard-links the other launcher's files.
* **Fill gaps:** missing files are downloaded from the official Mojang
  endpoints.
* **Atlas-native runtime (future):** a `Preparer` takes the user's own client
  jar (verified against the pinned hash), extracts what the runtime needs
  into `versions/<id>/prepared/`, writes a stamp file with input hashes, and
  holds a lock file during the work. Progress events go through the same
  channel as downloads, and the job can be cancelled.

## 9. Microsoft sign-in (Phase 3 design)

1. OAuth 2.0 **authorization code with PKCE** in the **system browser**, with
   a loopback redirect (`http://127.0.0.1:<random>/callback`) and a `state`
   check. Atlas never shows a password field. Device-code flow is the
   fallback.
2. The Microsoft token is exchanged for Xbox Live, then XSTS, then the
   Minecraft services token. Atlas then checks the entitlements (ownership)
   and the profile.
3. **Storage:** the refresh token lives in the OS credential store via the
   `keyring` crate (Windows Credential Manager, macOS Keychain, Secret
   Service on Linux). `accounts/` only holds non-secret metadata. If no
   secure store is available, Atlas refuses to remember the account; it
   never falls back to a plaintext file.
4. **Multiple accounts:** stored by opaque Atlas id, with the active one in
   `accounts.active_account`. Logout removes the keyring entry and the
   metadata.
5. **Refresh:** tokens are refreshed before expiry. A failed refresh yields
   `AUTH_EXPIRED` with a "Sign in again" suggestion.
6. Tokens are wrapped in a `Secret` newtype with a redacting `Debug`, and the
   log redaction patterns act as a second safety net.

**Operational requirement:** this needs an Azure AD application (client id).
Microsoft also requires new third-party apps to be approved before they can
call `api.minecraftservices.com`. Atlas cannot ship working sign-in until
that approval exists.

## 10. Installations and launching (Phase 3 design)

* `installations/<id>/installation.toml` holds: id (UUID), name, runtime
  (`vanilla-java`), Minecraft version, Atlas build (optional), game dir
  (default `installations/<id>/game`, or an explicit external path), world
  dir, Java path, memory, JVM args, game args, and resolution.
* **Operations:**
  * create / rename / duplicate. Duplicate copies with `copy_tree_no_follow`.
  * repair = verify the version.
  * update = change the version and verify.
  * delete needs a `Confirmation`, removes **only** `installations/<id>/`,
    refuses if the game dir is external, and never follows symlinks.
* **Launch service:** the backend state machine is
  NoInstallation → NotInstalled → Installing/Preparing/Updating → Ready →
  Launching → Running{pid} → Stopping → Ready | Error{GAME_CRASHED}.
  * Launching builds the argument list from the version JSON rules (OS,
    arch, features).
  * It verifies files first, spawns the game with piped stdout/stderr, and
    streams them into the log buffer (source `game`).
  * A non-zero exit, or a new file in `crash-reports/`, produces
    `GAME_CRASHED` with the crash file attached.
  * Stop sends a polite terminate, then a kill after a timeout.
  * The `GameState` enum already exists in `atlas-core`, and the Play button
    label comes from it.

## 11. Servers, content, worlds (Phase 4 design)

* **Servers:** `servers.toml` in the data root (name, address, port, icon,
  version, last played, favourite). Ping uses Server List Ping. Connect means
  launch the selected installation with `--quickPlayMultiplayer`.
* **Content:** scans `mods/`, `resourcepacks/` and `shaderpacks/` in the
  installation's game dir.
  * Disabling renames a file to `*.disabled`.
  * Atlas reads metadata (`fabric.mod.json`, `META-INF/mods.toml`,
    `pack.mcmeta`) for version and dependency info.
  * Modpack import (`.mrpack`) extracts through `safe_join`, enforces
    size/entry-count limits, refuses symlink entries and executables
    outside allowed folders, and verifies every listed hash.
* **Native mods:** `ContentKind` has room for an `AtlasNative` kind. The UI
  never claims that Fabric, Forge or NeoForge Java mods run on a native
  runtime.

## 12. CLI (implemented in part)

`atlas` uses the same `Atlas` facade as the GUI.

* **Today:** `status`, `paths`, `settings show|get|set|reset --yes`,
  `data-dir show|move --yes`, and `logs [--level] [--search] [--source]
  [-n]`, all with `--json`.
* **Later phases:** `versions`, `install`, `remove`, `prepare`, `play`,
  `stop`, `instances`, `servers`, `mods`, `account`.

## 13. MCP (Phase 5 design)

* **Off by default.** It needs both `advanced.ai.enabled` (the global kill
  switch) and `advanced.ai.mcp_enabled`. When the kill switch is off, the
  MCP crate refuses to start, whatever else is configured.
* **Transport:** stdio, launched by the client as `atlas mcp`, or
  `127.0.0.1` only. HTTP requires a random bearer token stored in a
  user-only (0600) file.
* **Tools:** `atlas_versions`, `atlas_install`, `atlas_prepare`,
  `atlas_play`, `atlas_stop`, `atlas_instances`, `atlas_servers`,
  `atlas_mods`, `atlas_logs` and `atlas_settings`. Each is a thin wrapper
  over `Atlas`.
* **Destructive tools** (remove, delete, reset, data-dir move) return
  `CONFIRMATION_REQUIRED` with a one-time nonce. Only the user can approve
  it, in the GUI or terminal; the model cannot self-approve.
* **No tool can read account tokens**, because the `Atlas` API exposes none.
  Log output is redacted.

## 14. Self-update (Phase 5 design)

* **Check:** fetch a release manifest over HTTPS (version, per-platform URL,
  SHA-256, size), plus a detached **Ed25519 signature**. Verify it against
  the public key compiled into Atlas. Reject if the signature is missing or
  invalid, or if the version would be a downgrade.
* **Download** with resume, then verify the SHA-256.
* **Stage** into `updates/<version>/`.
* **Replace:** atomic rename-swap of the executable (on Windows, the
  rename-self trick), keeping `previous/` for rollback.
* **Restart**, then the new version writes a health marker after a
  successful start. If no marker appears within N seconds on the next run,
  Atlas **rolls back** automatically.
* Game builds (`AtlasBuildSource`) use the same signature scheme.

## 15. Security checklist

| Threat | Mitigation |
|---|---|
| Path traversal | `fs_safety::safe_join` rejects `..`, absolute paths, drive prefixes and NUL (tested). |
| Malicious archives | Paths go through `safe_join`, with entry/size limits, no symlink entries, and no executable bits outside allowed folders. |
| Corrupted downloads | Mandatory hash and size check, `.part` files, atomic rename. |
| Symlink attacks | Copy, measure and remove helpers never follow links, and Atlas refuses to delete through a symlinked root (tested). |
| Token leaks | Keyring storage, `Secret` newtype, redaction on every log path (tested), no token APIs exposed to MCP. |
| Unsafe updates | Ed25519 signature with a pinned key, hash check, staging, rollback. |
| Accidental deletion | `Confirmation` is required by the backend, so every front-end must obtain it. Deletes are scoped to Atlas-owned paths. |
| Unsafe code | `unsafe_code = "forbid"` across the workspace. |

## 16. How Atlas differs from Horizon

| | Horizon (public docs) | Atlas |
|---|---|---|
| Names | `horizon-*` binaries, `HORIZON_HOME` | `atlas-launcher`, `atlas`, `ATLAS_CONFIG_DIR` / `ATLAS_DATA_DIR` |
| Config | its own | `settings.toml` (schema 1) + `locator.toml` |
| Data dir | its own | `atlaslauncher` / `AtlasLauncher` |
| Runtime | its own native client | runtime abstraction; vanilla Java first |
| UI | its own | original egui UI: "night-chart" palette, procedural globe mark, contour hero, hand-drawn icons |
| Update system | its own | Atlas Ed25519 manifest + health-marker rollback |

## 17. Phases

1. **Done:** workspace, window, navigation, settings, configuration,
   logging, data dirs, data-root move, CLI base.
2. **Done:** version metadata, downloads, verification, installation,
   repair, removal, update check, detection and reuse.
3. Microsoft auth, accounts, installations, process launching, game logs.
4. Servers, mods, resource packs, worlds, full CLI.
5. MCP, self-update, packaging and installers, crash handling.
