# Forever Sync

Keeps World of Warcraft: Forever settings (`_forever_/WTF`) the same on all your Macs and Windows PCs, using git and a private GitHub repo you own. Addons sync as a WowUp import list; Forever Sync never touches addon files.

Not affiliated with Blizzard Entertainment.

- Design: [docs/DESIGN.md](docs/DESIGN.md)
- GitHub App setup: [docs/github-app-setup.md](docs/github-app-setup.md)

## Layout

```
apps/desktop/            Tauri 2 app (React + TS UI in src/, Rust shell in src-tauri/)
crates/foreversync-core     Forever install discovery, rules, sync engine
crates/foreversync-addons   WowUp reader + import string (no addon file handling)
crates/foreversync-github   GitHub App config, device flow (next)
crates/foreversync-git      bare-repo storage via libgit2
crates/foreversync-cli      `foreversync` developer CLI
```

## Development

Requires Rust (stable), Node 22+ and pnpm.

```sh
cargo test --workspace
cargo run -p foreversync-cli -- detect          # find the Forever install (read-only)
cargo run -p foreversync-cli -- addons          # WowUp addon list; --import-string prints the string
cargo run -p foreversync-cli -- status          # WTF/ vs latest snapshot
cargo run -p foreversync-cli -- snapshot        # commit WTF/ into the local data repo
cargo run -p foreversync-cli -- log
cargo run -p foreversync-cli -- --root <copy> apply --dry-run   # write a snapshot back (backs up replaced files)
cd apps/desktop && pnpm install && pnpm tauri dev
```

The CLI's data folder defaults to `~/Library/Application Support/Forever Sync` (macOS) or `%LOCALAPPDATA%\Forever Sync` (Windows); pass `--data-dir` to use another. `apply` refuses to run while the Forever client is running.
