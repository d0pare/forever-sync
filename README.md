# Forever Sync

Keeps World of Warcraft: Forever settings (`_forever_/WTF`) the same on all your Macs and Windows PCs, using git and a private GitHub repo you own. Addons sync as a WowUp import list; Forever Sync never touches addon files.

Not affiliated with Blizzard Entertainment.

- Design: [docs/DESIGN.md](docs/DESIGN.md)
- GitHub App setup: [docs/github-app-setup.md](docs/github-app-setup.md)

## Layout

```
apps/desktop/            Tauri 2 app: onboarding, dashboard, tray, background sync (React UI in src/, Rust in src-tauri/)
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

The desktop app keeps running in the tray when its window is closed (quit from the tray menu). It syncs a few seconds after WoW Forever exits and every 10 minutes; while the game runs it only uploads, and applies incoming changes after you quit. Release builds turn on start-at-login after setup; debug builds leave that to the Settings toggle.

The CLI's data folder defaults to `~/Library/Application Support/Forever Sync` (macOS) or `%LOCALAPPDATA%\Forever Sync` (Windows); pass `--data-dir` to use another. `apply`, `restore` and `sync` never write into WTF/ while the Forever client is running. `--token-file` (or `FOREVERSYNC_TOKEN_FILE`) keeps tokens in a file instead of the keychain, for development.

## Standalone app

```sh
cd apps/desktop
pnpm bundle:mac   # macOS → target/release/bundle/macos/Forever Sync.app and dmg/Forever Sync_<ver>_aarch64.dmg
pnpm bundle:win   # Windows → target/release/bundle/nsis/Forever Sync_<ver>_x64-setup.exe (installs per user, no admin)
```

- **macOS:** drag `Forever Sync.app` to `/Applications` (or open the `.dmg`). The app is ad-hoc signed, which is enough on the Mac that built it. Another Mac blocks a downloaded copy until it is signed with an Apple Developer ID and notarized (DESIGN.md §11, S6). Until then, right-click → Open, or build on that Mac.
- **Windows:** run the setup `.exe`. Unsigned installers trigger SmartScreen when downloaded ("More info" → "Run anyway"). Builds made on the same PC don't.
- OpenSSL (used by libgit2 for HTTPS on macOS) is compiled in, so the app doesn't depend on Homebrew.
