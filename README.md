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
crates/foreversync-git      git storage via libgit2 (M1)
crates/foreversync-cli      `foreversync` developer CLI
```

## Development

Requires Rust (stable), Node 22+ and pnpm.

```sh
cargo test --workspace
cargo run -p foreversync-cli -- detect          # find the Forever install (read-only)
cargo run -p foreversync-cli -- addons          # WowUp addon list; --import-string prints the string
cd apps/desktop && pnpm install && pnpm tauri dev
```
