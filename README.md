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
crates/foreversync-github   device flow, token refresh/storage, repo setup
crates/foreversync-git      bare-repo storage via libgit2
crates/foreversync-cli      `foreversync` developer CLI
```

## Development

Requires Rust (stable), Node 22+ and pnpm.

```sh
cargo test --workspace                          # add `-- --ignored` to also test the real OS keychain
alias fs='cargo run -q -p foreversync-cli --'
fs detect                                       # find the Forever install (read-only)
fs addons [--import-string]                     # WowUp addon list / import string
fs login                                        # GitHub device-code sign-in (tokens go to the OS keychain)
fs setup [--repo owner/name]                    # pick the private data repo
fs sync [--first-sync use-cloud|upload-local] [--policy newest|local|remote]
fs status | snapshot | log | apply | restore <rev>
cd apps/desktop && pnpm install && pnpm tauri dev
```

The CLI's data folder defaults to `~/Library/Application Support/Forever Sync` (macOS) or `%LOCALAPPDATA%\Forever Sync` (Windows); pass `--data-dir` to use another. `apply`, `restore` and `sync` never write into WTF/ while the Forever client is running. `--token-file` (or `FOREVERSYNC_TOKEN_FILE`) keeps tokens in a file instead of the keychain, for development.
