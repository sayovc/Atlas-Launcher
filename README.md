# Atlas Launcher

An original, open Rust desktop launcher for Minecraft: Java Edition.

> **Status: pre-alpha (Phase 2 of 5 done).** The window, settings, logging,
> data folders, CLI, and installing/verifying/repairing official Minecraft
> versions work. Signing in, launching, mods and servers are **not built yet**. The UI says so instead
> of showing fake data.
>
> Atlas is not affiliated with Mojang, Microsoft or any other launcher. It
> ships no Mojang assets or code. You need your own legally obtained copy of
> Minecraft.

![Home](docs/screenshots/home.png)

## Build and run

Needs Rust 1.85 or newer.

```bash
cargo run -p atlas-ui --bin atlas-launcher   # desktop app
cargo run -p atlas-cli --bin atlas -- status # CLI
cargo test --workspace
```

On Linux the window needs the usual X11/Wayland runtime libraries (for
example `libxkbcommon-x11-0` on Debian/Ubuntu).

## What works today

| Area | State |
|---|---|
| Desktop window, sidebar navigation, animations, dark theme, 4 accents | ✔ |
| Settings (9 sections), typed + validated, atomic save, corrupt-file backup | ✔ |
| Data folder in platform dirs, relocatable with a verified move | ✔ |
| Structured logging (JSON files, rotation, redaction), live log viewer | ✔ |
| Structured errors (code, message, details, suggestion) | ✔ |
| CLI: `status`, `paths`, `settings`, `data-dir`, `logs`, `--json` | ✔ |
| Version list from Mojang, install / verify / repair / remove, update check | ✔ |
| Resumable, checksum-verified, cancellable downloads; reuse from existing `.minecraft` | ✔ |
| Installations, Microsoft sign-in, launching | Phase 3 |
| Servers, mods, worlds | Phase 4 |
| MCP, self-update, installers | Phase 5 |

## CLI examples

```bash
atlas status
atlas settings set memory.max_mb 6144
atlas settings get memory.max_mb --json
atlas data-dir move /mnt/games/atlas --yes
atlas logs --level warn --search download -n 50
atlas versions --refresh
atlas install latest
atlas verify 26.3
atlas repair 26.3
atlas remove 26.3 --yes
atlas detect
```

Destructive commands refuse to run without `--yes` (exit code 3,
`CONFIRMATION_REQUIRED`).

## Documentation

* [Architecture](docs/ATLAS_ARCHITECTURE.md)

## License

MIT — see [LICENSE](LICENSE).
