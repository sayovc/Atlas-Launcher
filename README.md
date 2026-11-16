# Atlas Launcher

An original, open Rust desktop launcher for Minecraft: Java Edition.

> **Status: pre-alpha (Phase 1 of 5).** The window, navigation, settings,
> logging, data folders and the CLI basics work. Signing in, installing,
> launching, mods and servers are **not built yet**. The UI says so instead
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
| Versions, downloads, installations, Microsoft sign-in, launching | Phase 2–3 |
| Servers, mods, worlds | Phase 4 |
| MCP, self-update, installers | Phase 5 |

## CLI examples

```bash
atlas status
atlas settings set memory.max_mb 6144
atlas settings get memory.max_mb --json
atlas data-dir move /mnt/games/atlas --yes
atlas logs --level warn --search download -n 50
```

Destructive commands refuse to run without `--yes` (exit code 3,
`CONFIRMATION_REQUIRED`).

## Documentation

* [Architecture](docs/ATLAS_ARCHITECTURE.md)

## License

MIT — see [LICENSE](LICENSE).
