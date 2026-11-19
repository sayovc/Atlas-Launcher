# How Atlas works

Notes on how the pieces fit together. Read this before changing anything
big.

## The short version

All the logic lives in `atlas-core`. The desktop app (`atlas-ui`), the
command line (`atlas-cli`) and the MCP server (`atlas-mcp`) each hold an
`Atlas` and call methods on it. None of them make decisions of their own.
That's why something that works in the CLI also works in the app: it's
the same code.

```
atlas-ui ──┐
atlas-cli ─┼─► atlas-core ─► versions, downloads, instances, launch,
atlas-mcp ─┘                 auth, content, updater
                             └─► config, logging, shared
```

## Crates

| Crate | What's in it |
|---|---|
| `atlas-shared` | The error type, log redaction, safe path and file helpers |
| `atlas-config` | Where things live on disk, and `settings.toml` |
| `atlas-logging` | Log files, the in-memory buffer for the log viewer |
| `atlas-downloads` | Resumable, checksum-verified, cancellable downloads |
| `atlas-versions` | Mojang's version list, installing/verifying/repairing versions, mod loaders, Java runtimes, finding other launchers |
| `atlas-instances` | Installations |
| `atlas-launch` | Building the java command line, running the game, crash detection |
| `atlas-auth` | Microsoft → Xbox → Minecraft sign-in, saved accounts |
| `atlas-content` | Modrinth (search, installs, updates), bundles, servers, mods, packs, worlds, `.mrpack` import |
| `atlas-updater` | Signed self-update with rollback |
| `atlas-mcp` | The MCP server |
| `atlas-core` | Ties it all together |
| `atlas-ui` | The app (egui) |
| `atlas-cli` | `atlas` |
| `xtask` | Release helpers: signing key, update manifest |

The UI uses egui/eframe. It's pure Rust, builds quickly everywhere, and
runs under a virtual display, so the UI can be screenshot-tested in CI.
The UI crate is separate, so it could be swapped later without touching
the backend.

## Files on disk

There are two folders, found through the `directories` crate:

- **Bootstrap folder** (`~/.config/atlaslauncher`, `%APPDATA%\AtlasLauncher`,
  `~/Library/Application Support/AtlasLauncher`). It holds `locator.toml`,
  which says where the data folder is, and the MCP token.
- **Data folder** (by default `~/.local/share/atlaslauncher` on Linux,
  the same as above on Windows/macOS). It holds everything else:

```
settings.toml        servers.toml
accounts/            names and ids only, tokens are in the keychain
installations/<id>/  installation.toml, game/ (saves, mods, ...)
versions/<id>/       version json, client jar, install.toml
versions/.staging/   installs in progress
libraries/ assets/   shared by all versions
runtimes/            Java from Mojang
backups/             world backups
cache/  logs/  updates/  mcp/
```

`ATLAS_CONFIG_DIR` and `ATLAS_DATA_DIR` override both, which is handy for
portable installs and tests.

Atlas reads other launchers' folders (to reuse files it can verify), but
never writes to them.

**Moving the data folder.** The target must be empty. Atlas copies its own
folders, checks file counts and sizes, points the locator at the copy, and
only then deletes the originals. Symlinks are skipped rather than
followed.

## Errors

Every error has four parts:

- a code (`CHECKSUM_MISMATCH`, `AUTH_EXPIRED`, `GAME_CRASHED`, ...)
- a sentence for the user
- technical details (always redacted)
- a suggestion

The app shows them as a banner. The CLI prints them, or outputs
`{"ok":false,"error":{…}}` with `--json`. CLI exit codes: 2 means a bad
argument, 3 means "needs `--yes`", and 1 is anything else.

## Installing a version

1. **Manifest.** Fetch Mojang's manifest over HTTPS and cache it, so the
   version list still works offline.
2. **Version JSON.** Download it and check it against the SHA-1 in the
   manifest.
3. **The rest.** Work out the jar, libraries (respecting OS rules), legacy
   natives, asset index, assets and logging config, and check each against
   the hash the previous file gave.
4. **Download.** Everything goes into `.part` files and is renamed into
   place only once its hash matches. Shared files go straight to
   `libraries/` and `assets/`. The version's own files go into
   `versions/.staging/<id>/`.
5. **Finish.** Write `install.toml` (every file with its hash, size and
   URL), then rename staging to `versions/<id>/`. A version only counts as
   installed once that file exists, so an interrupted install never looks
   finished.

Verify re-hashes everything in `install.toml`. Repair re-downloads only
the bad files.

## Playing

`Atlas::play` runs in the background and moves through these states:
Installing (if needed), then Preparing (Java), Launching, Running, and
finally Exited or Error.

- **Java:** comes from Mojang's runtime index, with every file hashed.
  Links in the runtime must stay inside it.
- **Command line:** built from the version JSON. It handles both the
  modern `arguments` and the old `minecraftArguments` formats, applies OS
  and feature rules (demo mode, custom resolution, joining a server), and
  falls back to `--server` on old versions.
- **Game output:** shows up in the log viewer tagged `game`.
- **Crashes:** a non-zero exit code, or a new file in `crash-reports/`,
  counts as a crash. The report is shown in the logs.
- **Stopping:** Stop asks politely, then force-kills after 10 seconds.
  `atlas stop` from another terminal uses a PID file and checks that the
  PID is actually a Java process before touching it.

The Play button only ever shows what `Atlas::game_state()` returns.

## Mod loaders

A loader is a small version file that builds on a vanilla version
(`inheritsFrom`) and adds libraries, a main class and some arguments.
Atlas installs that file like any other version, under
`versions/<profile>/` with its own `install.toml`, so verify and repair
work the same way. The launcher merges it onto its parent at start time:

- loader libraries go first, and a loader's library replaces vanilla's
  copy of the same one (asm, for example)
- the loader's main class is used
- arguments are combined
- assets and Java come from the parent

Where each loader comes from:

- **Fabric:** Fabric's official metadata, which includes a SHA-1 and size
  for every library.
- **Quilt:** Quilt's official metadata. There are no hashes in the profile,
  so Atlas reads the `.sha1` file next to each jar on Quilt's Maven.
- **Forge and NeoForge:** Modrinth's launcher-meta, the open "daedalus"
  format, which has already unpacked the installer. Atlas then runs the
  client-side installer steps ("processors") with Java and checks their
  outputs. The binary patch file in those profiles has no published hash,
  so Atlas takes it from the official installer jar instead, which has a
  `.sha1` on Forge's and NeoForge's own Maven.

Using a metadata service is the idea that keeps this manageable; it's the
same approach OneLauncher takes with its fork of daedalus.

## Modrinth and bundles

- **Search:** filtered to the installation's loader and Minecraft version.
- **Install:** picks the newest release (betas only if there's no
  release), follows required dependencies, checks every file's SHA-512,
  and only downloads from Modrinth's CDN. Everything installed is recorded
  in `<game>/.atlas/content.json`, so updates and removal know what's
  what.
- **Updates:** one request to Modrinth's `version_files/update` with the
  hashes of what's installed.
- **Bundles:** curated lists of Modrinth slugs (all Fabric). They're
  resolved per Minecraft version, and anything without a build is skipped
  and reported, never swapped for something else. "Create & play" is a
  Fabric installation, plus bundles, plus play once they're in.

## Signing in

1. **Microsoft.** Browser sign-in uses OAuth with PKCE and a temporary
   `localhost` callback (state checked), with a device-code flow as the
   fallback.
2. **Xbox Live and XSTS.** Known XSTS error codes are turned into readable
   messages: no Xbox profile, child account, region.
3. **Minecraft.** Log in to Minecraft services, then check ownership
   through entitlements and the player profile. Game Pass accounts show up
   as having a profile.
4. **Storage.** The refresh and game tokens go to the OS keychain via the
   `keyring` crate. If there's no keychain, Atlas won't save the account.
   It never writes tokens to a plain file. Tokens are refreshed before
   they expire.

The Azure app id comes from `ATLAS_MS_CLIENT_ID`, at build time or at
runtime. Until it's set, sign-in says so and the game runs in demo mode.

## Content

- **Servers:** live in `servers.toml`. The status ping is the same one the
  game's multiplayer screen uses.
- **Mods:** read from `mods/`. Disabled mods are renamed to
  `.jar.disabled`, and missing dependencies are flagged.
- **Worlds:** the name and mode come from `level.dat`, using a small NBT
  reader with depth and size limits. Backups are zip files, symlinks are
  skipped, and deleting a world always backs it up first.
- **`.mrpack` import:** the installation gets the pack's loader and loader
  version. Downloads are only allowed from Modrinth, GitHub and GitLab, every file's SHA-512 or SHA-1 is checked, and overrides are
  unpacked with size limits and no symlinks. Every path stays inside the
  game folder.

## MCP

`atlas mcp` speaks MCP over stdin/stdout.

- **Off by default:** it only runs while both `advanced.ai.enabled` and
  `advanced.ai.mcp_enabled` are on, and it checks that on every request.
- **Token:** the caller has to pass the token from the `mcp-token` file
  (user-only permissions) in `ATLAS_MCP_TOKEN`.
- **No account access:** no tool returns account tokens, and the settings
  tool can't change the AI or account settings.
- **Approval codes:** destructive tools return a one-time code tied to the
  exact action. The user approves it with `atlas mcp approve <code>`.

## Self-update

A release has `atlas-update.json` plus an Ed25519 signature for it.

- **Signature:** Atlas only trusts the public key it was built with
  (`ATLAS_UPDATE_PUBKEY`).
- **Version:** it refuses older versions.
- **Install:** the binary's SHA-256 is checked, the old binary is backed
  up, and then the new one is swapped in.
- **Rollback:** if the new version fails to start three times, the old one
  comes back. A start counts as good after 10 seconds of normal running.

Builds without a key just say updates aren't set up.

## Security checklist

| Risk | What stops it |
|---|---|
| Path traversal | `safe_join` everywhere paths come from outside (tested) |
| Malicious archives | Size and entry limits, no symlinks, paths through `safe_join` |
| Corrupted downloads | Mandatory hashes, `.part` files, atomic rename |
| Symlink tricks | Copy and delete helpers never follow links |
| Token leaks | Keychain only, redaction on every log path, `Secret` hides itself in debug output |
| Bad updates | Signature with a pinned key, hash check, rollback |
| Accidental deletion | The backend requires confirmation; deletes stay inside Atlas's own folders |

The workspace uses `unsafe_code = "forbid"`.

## What's not done yet

- CurseForge (its API needs a key).
- Pre-made mod configs for the bundles; mods start with their own defaults.
- Code signing and notarization for Windows and macOS builds.
- Working sign-in needs the Azure app id. The approval for the Minecraft
  API is tied to that id.
