# Forever Sync: Design

Status: Draft v0.3 (2026-11-18). **Scope: World of Warcraft: Forever only, permanently.**

Forever Sync is a desktop app for macOS and Windows. It keeps a player's **WoW Forever** settings (UI, addon SavedVariables, keybindings, macros) and installed addons the same on every machine they use. Git does the versioning, a private GitHub repository holds the data, and users sign in with GitHub.

Forever Sync supports only the Forever client (`_forever_`, product `wowf`, patch 1.60.x). Other WoW versions (Retail, Classic, PTR) are **out of scope by design**, so nothing is abstracted over "flavors". Forever-specific facts sit in one constants module (§3.6) because the client is new and its conventions may still change.

---

## 1. Goals and non-goals

### Goals
1. **Sync Forever settings** (`_forever_/WTF/`) across machines: account-wide and per-character.
2. **Sync the installed addon set through WowUp**: keep the list of WowUp-managed addons in the repo and hand it to WowUp as an import string. Forever Sync never stores, copies, downloads or deletes addon files.
3. **Full history**: every snapshot is a git commit, and users can roll back one addon's settings or everything.
4. **GitHub as storage**: a GitHub sign-in sets up a private repo. There is no backend of our own.
5. **Never lose data**: never write while the Forever client is running, back up before every overwrite, and make everything recoverable.
6. **Low effort after setup**: runs in the tray and syncs when the game exits and before you play.

### Non-goals (v1)
- Any WoW version other than Forever (Retail, Classic, Classic Era, PTR, Beta). Not planned.
- Linux.
- Replacing an addon manager, or downloading addons ourselves. **WowUp installs and updates addons.** Forever Sync syncs the *list* and hands it to WowUp as an import string (§6). We do not integrate with CurseForge or any other addon API.
- Merging two edits to the same SavedVariables file at the Lua level (v1 resolves conflicts per file; §7.4).

---

## 2. What a real Forever install looks like

These measurements come from a real macOS install on 2026-11-18: client `1.60.1.70938`, 23 addon packages installed through WowUp-CF.

```
/Applications/World of Warcraft/
  .build.info              Product = "wowf", Version = 1.60.1.70938
  .product.db              protobuf; product "wowf", subfolder "_forever_"
  World of Warcraft Launcher.app
  _forever_/
    .flavor.info           "Product Flavor!STRING:0\nwowf"
    World of Warcraft.app  ← same bundle name as the Retail client!
    Interface/AddOns/      3.9 GB, ~31,200 files (21k .ogg, 2k .mp3, 3k .blp)
    WTF/                   4.4 MB
    Fonts/                 32 MB of *.slug / *.slugo, a client-generated glyph cache, not user fonts
    Cache/  Logs/  Utils/
```

**WTF layout** (names generalized):
```
WTF/
  Config.wtf                          machine-specific + sensitive
  GamePadConfig_Default.json          controller config
  gamecontrollerdb.txt                controller mappings
  SavedVariables/                     GLOBAL, login-screen state (Console history, Glue, AddOnList)
  Account/
    SavedVariables/                   account-agnostic Blizzard settings (SavedSets, Settings, Gamepad)
    <ACCOUNT>/                        plain uppercase name (NOT the "12345678#1" form Retail uses)
      SavedVariables/*.lua (+ .lua.bak)
      bindings-cache.wtf  macros-cache.txt  config-cache.wtf
      edit-mode-cache-account.txt  tts-cache-account.txt  flagged-cache-account.txt
      chat-frontend-cache.txt  character-list-order.txt
      *.old  cache.md5
      <REALM-ID>/                     NUMERIC realm id (e.g. "93"), not a realm name
        <Character-Folder>/
          AddOns.txt                  per-character addon enable state
          SavedVariables/             (only if an addon declares per-character SVs)
```

Important for the design:
- **Addons are about 900× larger than settings (3.9 GB vs 4.4 MB) and almost entirely voice-over audio** (Spoken packs make up about 3.5 GB). Committing addon files to git, which v0.1 proposed, **cannot work** on GitHub: the soft repo limit is about 1 GB, a single push is capped at 2 GB, and individual QuestieDB `.toc` files reach 57 MB. Addons have to sync as a **manifest**, with files stored only for addons that cannot be reinstalled from a source (§6).
- **All 23 installed packages are known to WowUp-CF**, with provider, project id, release id, version and folder list. That is exactly what WowUp's own Import/Export feature needs, so Forever Sync can generate a WowUp import string and let WowUp do the installing.
- The `.app` bundle is named `World of Warcraft.app` for Forever **and** Retail, so process detection must match by path (§3.3).

---

## 3. Forever platform facts

### 3.1 Install discovery
1. `product.db` (protobuf): `/Users/Shared/Battle.net/Agent/product.db` on macOS, `C:\ProgramData\Battle.net\Agent\product.db` on Windows. Find entries with product **`wowf`** and read the install path and subfolder (`_forever_`).
2. Fallback: look in the default roots (`/Applications/World of Warcraft`, `C:\Program Files (x86)\World of Warcraft`, `C:\Program Files\World of Warcraft`) for a `_forever_` folder.
3. Manual folder picker.

A folder counts as Forever when `<root>/_forever_/.flavor.info` contains `wowf`. Read the client version from `<root>/.build.info` (the row whose `Product` is `wowf`) and record it in each snapshot.

### 3.2 When the client reads and writes files
- SavedVariables are read when the UI loads after character select, and written on logout, `/reload` and quit.
- `Config.wtf` is read at startup and written at exit.
- So **reading is safe at any time once a file has stopped changing**, and **writing is safe only while the Forever client is not running**.

### 3.3 Is Forever running?
Use path-based matching (`sysinfo`), because the executable name alone is ambiguous:
- **macOS:** a process whose executable is `<root>/_forever_/World of Warcraft.app/Contents/MacOS/World of Warcraft` (or any executable under the `_forever_` folder).
- **Windows:** a process whose image path is under `<root>\_forever_\`. The exe name is still **unverified** (spike S4), so we don't hard-code it.

As a result, a Retail client running from the same root does **not** block Forever sync, and the reverse holds too.

### 3.4 Addon TOC facts
- Forever `## Interface` = **16001** (patch 1.60.1).
- Multi-client addons ship flavor-specific TOCs. Both **`_Forever`** and **`_Camelot`** suffixes appear in the wild ("Camelot" was Forever's internal codename), next to `_Vanilla`, `_TBC`, `_Mists`, `_Mainline` and others.
- The scanner treats **every** `*.toc` in a folder as metadata for that folder, prefers `_Forever`/`_Camelot` over the base TOC when reading the title and version, and ignores the others.
- Package hints found in TOCs: `## X-Curse-Project-ID`, `## X-Wago-ID`, `## X-WoWI-ID`, `## X-Part-Of`, `## X-Child-Of`, `## Dependencies` / `## RequiredDeps`.

### 3.5 What gets synced (Forever defaults)

| Path (relative to `_forever_/`) | Default | Why |
|---|---|---|
| `WTF/Account/<ACCT>/SavedVariables/*.lua` | ✅ | Main value |
| `WTF/Account/<ACCT>/<REALM>/<CHAR>/SavedVariables/*.lua` | ✅ | |
| `WTF/Account/<ACCT>/<REALM>/<CHAR>/AddOns.txt` | ✅ | Which addons each character has enabled |
| `WTF/Account/<ACCT>/*.wtf`, `*-cache*.txt`, `macros-cache.txt`, `character-list-order.txt` | ✅ | Bindings, macros, edit mode, TTS, chat. Blizzard may also sync some server-side, which does no harm. |
| `WTF/Account/SavedVariables/*.lua` | ✅ | Small, account-agnostic Blizzard settings |
| `WTF/SavedVariables/*` (global) | ❌ | Login-screen state and console history (100 KB, machine-local) |
| `WTF/Config.wtf` | ❌ | GPU and resolution, plus `accountName` (login email). v1.1: optional CVar allowlist. |
| `WTF/GamePadConfig_Default.json`, `gamecontrollerdb.txt` | ❌ (toggle) | Depends on the controller connected to each machine |
| `*.bak`, `*.old`, `cache.md5`, `*.tmp` | ❌ | Client-made backups and checksums |
| `Fonts/` | ❌ | Generated `.slug` glyph cache, rebuilt by the client |
| `Cache/`, `Logs/`, `Utils/`, `Screenshots/`, `Errors/` | ❌ | |
| `Interface/AddOns/` | ✅ as a **WowUp addon list only**, no files (§6) | |

Users can add exclusions per addon and per path glob. Guards: warn on any synced file over 10 MB and refuse files over 95 MB.

### 3.6 Forever constants
Every Forever-specific value above (subfolder, product code, `.flavor.info` value, interface numbers, TOC suffixes, process-path matcher, default include and exclude rules) lives in one module, `foreversync-core/src/forever.rs`, so a Blizzard layout change is a one-file fix.

---

## 4. Architecture

```
┌──────────────────────────── Tauri app ─────────────────────────────┐
│  UI (React + TS)                         Tray / notifications      │
│  onboarding · status · addons · history · conflicts · settings     │
│                │ Tauri commands + events                           │
│  ┌─────────────▼────────────────────────────────────────────────┐  │
│  │ foreversync-core (Rust)                                          │  │
│  │  discovery ─ forever consts ─ rules ─ scanner (stat cache)   │  │
│  │  settings sync engine (snapshot · merge · apply)             │  │
│  │  addon engine (WowUp inventory · manifest · import string)   │  │
│  │  triggers: process watcher · fs watcher · scheduler · backups│  │
│  └───────┬──────────────────────┬───────────────────┬───────────┘  │
│  ┌───────▼────────┐   ┌─────────▼────────┐   ┌──────▼──────────┐   │
│  │ foreversync-git    │   │ foreversync-github   │   │ foreversync-addons  │   │
│  │ libgit2 (git2) │   │ device flow, REST│   │ WowUp reader,   │   │
│  │ bare repo      │   │ keychain         │   │ TOC parser,     │   │
│  └───────┬────────┘   └─────────┬────────┘   │ import string   │   │
└──────────┼──────────────────────┼────────────┴─────────────────┴───┘
           ▼                      ▼
     github.com repo        api.github.com          (no addon downloads: WowUp does them)
```

### 4.1 Tech stack
| Concern | Choice | Why |
|---|---|---|
| Shell | **Tauri 2** | ~10 MB installers using the native webview. Official plugins for tray, autostart, updater and single-instance. |
| Core | **Rust** | `git2`, `notify`, `sysinfo`, `keyring`, `walkdir`, `base64`, `serde_json` |
| Git | **libgit2 via `git2`** (vendored) | No system git needed. Native TLS on both operating systems. |
| UI | React + TypeScript + Vite + Tailwind | |
| Secrets | `keyring` | macOS Keychain and Windows Credential Manager |
| Lua validation | `full_moon` | Never commit or apply a corrupt SavedVariables file |
| Local state | SQLite (`rusqlite`) | Stat cache, last applied tree, addon inventory cache |

### 4.2 Code layout
```
apps/desktop/           Tauri app (src/ React, src-tauri/ Rust shell)
crates/
  foreversync-core/     discovery, Forever constants, rules, scanner, sync + addon engines
  foreversync-git/      bare repo, tree build, 3-way merge, fetch/push
  foreversync-github/   device flow, token refresh, repo bootstrap
  foreversync-addons/   WowUp addons.json reader, TOC parser, package grouping, WowUp import string
  foreversync-cli/      headless CLI over core (dev, debugging, CI integration tests)
docs/
```

---

## 5. Repository model

### 5.1 The game folder is never a git working tree
The repo is a **bare** repo in app data (`~/Library/Application Support/Forever Sync/repos/<id>.git` or `%LOCALAPPDATA%\Forever Sync\repos\<id>.git`). The engine hashes files straight from `_forever_/` into git objects and writes blobs back when applying. No `.git` folder ever exists inside the game install, there is no duplicate copy on disk, and we control filtering, path normalization and write timing. A SQLite stat cache (`path → size, mtime, oid`) avoids rehashing unchanged files.

### 5.2 Layout
Single branch `main`, full history. It holds only settings and the addon list, so it should stay in the single-digit MB range (the reference `WTF/` is 4.4 MB before filtering).

```
foreversync.json                schema version, machines registry, repo settings
.gitattributes                  "* -text"   (byte-exact, no EOL conversion)
README.md                       what this is, and "keep this repo private"
wtf/Account/...                 filtered mirror of _forever_/WTF/Account
addons/manifest.json            installed addon set (§6.2)
addons/wowup-import.txt         ready-to-paste WowUp import string (§6.3)
rules.json                      synced exclusions
```

### 5.3 Commits
One commit per snapshot, with trailers:
```
Snapshot forever from "MacBook" (game-exit)

ForeverSync-Machine: 6f1c…
ForeverSync-Trigger: game-exit | manual | watch | schedule
ForeverSync-Client: wowf 1.60.1.70938
ForeverSync-Version: 0.1.0
```

### 5.4 Normalization
- Paths are stored as NFC with `/` separators.
- When applying, folders are matched case-insensitively against what exists on disk.
- `* -text` keeps file bytes identical on both operating systems.

---

## 6. Addon sync

**Forever Sync does no addon file handling at all.** It never stores, copies, downloads or deletes anything in `Interface/AddOns/`. WowUp stays the addon manager on every machine. Forever Sync keeps the **list** of WowUp-managed addons in the repo and turns it into a **WowUp import string**. On another machine you paste it into WowUp's *Import/Export Addons* dialog, and WowUp installs whatever is missing.

Addons that WowUp doesn't manage (hand-installed, or ones WowUp couldn't match) are **not synced**. The app lists them as "not managed by WowUp" so the user knows to add them to WowUp or install them by hand.

### 6.1 Inventory: what is installed
1. **WowUp records (primary):** `addons.json` in WowUp's app data folder: `~/Library/Application Support/WowUpCf/` on macOS, `%APPDATA%\WowUpCf\` on Windows (to verify, S3). Plain WowUp uses `WowUp/` instead of `WowUpCf/`. Read-only; we never write WowUp's files.
   - Pick the Forever installation from `preferences.json` → `wow_installations[]`, the entry with `clientType: 10` (WowUp's `WowClientType.Forever`). Use its `id` to filter `addons.json` records by `installationId`.
   - Useful record fields: `providerName` (`Curse`, `WowUpHub`, `GitHub`, `WowInterface`, `Wago`, …), `externalId`, `installedExternalReleaseId`, `installedVersion`, `installedFolders` (comma-separated), `isIgnored`, `installedAt`.
   - **Remove duplicate records:** the reference file has 2 stale duplicates for the same `(providerName, externalId)` with different versions. Keep the record whose folders exist on disk and whose `installedAt` is newest.
2. **Folder listing (read-only):** the folder names in `Interface/AddOns/`, plus each folder's TOC `## Title` for display. Nothing is hashed or copied. It is used only to:
   - confirm that a WowUp record's `installedFolders` exist on disk (this drops stale records),
   - flag folders that no WowUp record covers, or whose record is `isIgnored` (how WowUp marks addons it couldn't match), as **not managed by WowUp**. These are shown in the UI only and never synced.

### 6.2 Manifest (`addons/manifest.json`)
This is the source of truth. The 3-way merge (§7.3) runs over it, with one package per entry:
```json
{
  "key": "wowup:Curse:1076354",
  "name": "Enhance QoL",
  "wowup": { "provider": "Curse", "id": "1076354", "versionId": "7123456", "version": "13.23.0-forever" },
  "folders": ["EnhanceQoL", "EnhanceQoLBags", "..."],   // display + "is it installed here?" check
  "state": "installed"                  // or "removed" (tombstone)
}
```

### 6.3 WowUp import string
This is WowUp's own export format, read from WowUp-CF 2.25.5 (`getExportPayload` / `getImportSummary`):
```json
{
  "collection_name": "ForeverSync_<unix-ms>",
  "client_type": "Forever",
  "addons": [
    { "id": "1076354", "name": "Enhance QoL", "provider_name": "Curse", "version_id": "7123456" }
  ]
}
```
- The string is this JSON **base64-encoded**. WowUp also accepts raw JSON when the text starts with `{`.
- Built from the merged manifest (packages with `state: "installed"`), written to `addons/wowup-import.txt` on every commit so it can be copied from github.com even without the app, and shown in the app with a **Copy** button and an **Open WowUp** button.

How WowUp handles an import (this sets what we can promise):
| Situation on the receiving machine | WowUp's behavior |
|---|---|
| Addon not installed | **Installs it.** It installs the *latest* release on the default channel, not the exact `version_id`. |
| Installed with the same provider and version | No change |
| Installed but a different version or provider | Marked as a conflict and **left as it is** |
| `client_type` group doesn't match the selected installation | The whole import is rejected (`INVALID_CLIENT_TYPE`), so the user has to select Forever in WowUp first |
| Provider not enabled in that WowUp build (e.g. `Curse` in plain WowUp) | Rejected (`MISSING_CURSEFORGE_PROVIDER`). The app detects which WowUp build is installed and warns. |

In short, the import adds missing addons. It never removes them and never downgrades. Versions then converge, because WowUp keeps both machines on latest.

### 6.4 Status and removals
- After every sync, compare the manifest with this machine's WowUp inventory. Each addon gets a status: **in sync**, **missing** (needs WowUp import), **outdated or newer** (informational only) or **removed elsewhere**. Folders WowUp doesn't manage on this machine are listed separately as **not managed by WowUp**.
- **Missing addons** trigger one notification: "3 addons from MacBook aren't installed. Copy the WowUp import string?"
- **Removed elsewhere** (`state: "removed"` tombstone): shown as "remove in WowUp". Forever Sync never deletes addon folders.
- Addons WowUp installs show up in the next inventory scan (when `addons.json` changes), so the status clears without a manual step.

---

## 7. Sync engine

### 7.1 Triggers
| Trigger | Action |
|---|---|
| Forever client exits | Wait until files stop changing (about 3 s) → snapshot → fetch → merge → push → apply incoming |
| SavedVariables written while playing (`/reload`, logout to character select) | Snapshot and push. **Do not apply** incoming changes. |
| App start, wake from sleep, every 10 min while the client is closed | Fetch; if the remote is ahead → merge and apply |
| Changes in `Interface/AddOns/` or WowUp's `addons.json` | Re-inventory → update the manifest and import string → commit and push |
| "Play" button | Sync, then open the Battle.net launcher on Forever (best effort) |
| Manual "Sync now" | Full cycle |

### 7.2 Cycle (as implemented in `foreversync-core/src/sync.rs`)
```
1. fetch     origin/main → refs/remotes/origin/main
             (a machine with no history adopts the cloud's history first)
2. snapshot  scan WTF (rules + Lua check); local changes = diff(last OBSERVED disk state, scan)
             applied on top of main → commit if the tree changed
3. merge     fast-forward, or per-file 3-way merge commit (§7.3)
4. push      on non-fast-forward → fetch, merge again, retry (3 attempts)
5. apply     only while the client is not running: write main into WTF/,
             back up every overwritten or deleted file first (§7.5)
6. record    the scanned disk state under a local-only ref
             (refs/foreversync/disk/<install hash>; never pushed)
```

**Why local changes are measured against the last observed disk state, not `main`:** while the game runs, `main` can hold remote changes this machine hasn't applied yet. Diffing disk against `main` would quietly revert them, and would "delete" characters that only exist on another machine. Diffing against what this machine last saw on its own disk picks up only real local edits. If a local edit hits a file with an unapplied remote change, the local edit wins (it is newer) and is reported.

**First sync on a machine** (no observed disk state yet): `use-cloud` (default) keeps cloud versions where both sides have a file and adds local-only files. `upload-local` makes this machine's files win.

### 7.3 Merge rules
The unit is a **file** under `wtf/` and a **package** (manifest entry) in the addon manifest. Merging Lua tables line by line can produce invalid or semantically broken files, and WoW discards SavedVariables it cannot load.

| Local vs base | Remote vs base | Result |
|---|---|---|
| unchanged | changed | take remote |
| changed | unchanged | take local |
| changed | changed, same | take either |
| changed | changed, different | **conflict** → policy |
| deleted | unchanged | delete (only for paths Forever Sync tracked) |
| deleted | changed | keep the changed version |

Conflict policy: **newest snapshot wins, with a notification** (default; the loser is one click away in history), or always ask, or prefer this machine, or prefer the remote. A missing character or account folder is **never** read as a deletion.

### 7.4 Later: Lua-aware merge
Parse both sides into tables and merge by key path. Opt in per addon. Works well for AceDB-style `profiles` / `profileKeys` data.

### 7.5 Safety
- The local state is always committed (step 2) before anything is applied. Overwritten files are also copied to `backups/<timestamp>/` and kept for 14 days.
- Atomic writes: write a temp file, fsync, then rename.
- A `.lua` file that fails to parse is neither committed nor applied; it is flagged in the UI instead.
- A repo lock plus the Tauri single-instance plugin.
- First apply on a new machine always shows a preview.

---

## 8. GitHub integration

### 8.1 Auth: GitHub App + Device Flow (**decided** 2026-11-18)
- A GitHub App registered under the maintainer's personal account (setup steps in [github-app-setup.md](github-app-setup.md)). Device Flow is enabled, there is no webhook, and the app has no client secret or private key.
- Repository permissions: **Contents: read & write** and **Metadata: read**. **No Administration** permission: the user creates the repo and installs the app on it (§8.2).
- Device flow: show a code and open `github.com/login/device`. No client secret ships in the app.
- User access tokens are limited to repos where the user installed the app. They expire after 8 hours; the refresh token lasts 6 months. Refreshing a **device-flow** token needs no client secret, and each refresh rotates both tokens, so the new pair must be saved atomically.
- Tokens live in the OS keychain. Git over HTTPS uses user `x-access-token` with the token as password, supplied through a libgit2 credential callback.

*Rejected:* an OAuth App with the `repo` scope. It gives one-click setup but read/write access to every private repo the user has.

### 8.2 Repository bootstrap
1. Device flow sign-in.
2. `GET /user/installations` → accessible repos → look for `foreversync.json` at a repo root.
3. **Found:** this is an additional machine (§9).
4. **Not found (first machine):**
   1. Open the prefilled `https://github.com/new?name=forever-sync-data&visibility=private&description=…`. The user clicks *Create repository*.
   2. Open `https://github.com/apps/<slug>/installations/new`. The user picks *Only select repositories* → `forever-sync-data` → *Install*.
   3. The app polls `GET /user/installations/{id}/repositories` every few seconds until the repo appears, then checks that it is private and empty.
   4. Shortcut: if the user already has an installation with "All repositories", skip step ii.
5. Push the initial commit (`foreversync.json`, `.gitattributes`, README).

### 8.3 Privacy guard
- Check that the repo is private before every push (cached for 1 hour) and refuse to push to a public repo.
- The README warns that SavedVariables can contain chat and character data.
- Optional end-to-end encryption comes later.

---

## 9. User flows

### First machine
1. Sign in with GitHub (device code).
2. The app finds the Forever install and WowUp. It shows the account, characters (by realm id and folder name), settings size and addon inventory: "23 addons managed by WowUp (synced as a list), 0 not managed by WowUp (not synced)".
3. Confirm. The app creates the repo, takes the first snapshot and pushes.
4. The app moves to the tray, with autostart on.

### Additional machine
1. Sign in → the existing `forever-sync-data` is found.
2. The app finds the local Forever install.
3. **Initial reconcile:** choose **Use cloud** (recommended: back up local, take the repo state, keep local-only extras), **Merge** (3-way with an empty base), or **Upload this machine**.
4. Preview settings changes, then confirm and apply.
5. **Addons:** "23 addons to install in WowUp." Copy the import string, open WowUp, select the Forever install, go to *Import/Export Addons*, paste and install. Forever Sync shows a checklist that ticks off as WowUp installs each addon.

### Daily use
Play on machine A and quit; the snapshot is pushed within seconds. Machine B picks it up on wake or within 10 minutes and applies it. If A installed a new addon, B gets a "1 new addon, copy WowUp import" notification. Play on B.

### Restore
A history timeline. Restore everything, one addon's settings, one character, or the addon set as of a given date. Each restore is a new commit, so it can be undone too.

---

## 10. UI
1. **Onboarding:** sign in → detect → review (settings + addon list) → reconcile (additional machines) → done.
2. **Dashboard:** last sync, client running or not, pending apply, conflicts, repo size, Sync now.
3. **Addons:** WowUp addons with provider, version and status (in sync / missing / remove in WowUp / outdated). **Copy WowUp import** (all, or missing only) and **Open WowUp** buttons. Exclude toggle per addon. A separate "Not managed by WowUp (not synced)" section.
4. **History:** snapshot timeline with machine, trigger and client version. Diff summary and restore actions.
5. **Conflicts:** grouped by addon and character; keep either version.
6. **Settings:** install path, exclusions, conflict policy, schedule, autostart, account/repo, advanced (open repo, export diagnostics).
7. **Tray:** status, Sync now, Play, Open, Quit.

---

## 11. Risks and spikes (do these first)

| # | Spike | Question | Fallback |
|---|---|---|---|
| S1 | GitHub App device flow | Device-code sign-in, token refresh without a client secret, repo access after install-on-one-repo (test script in [github-app-setup.md](github-app-setup.md) §3) | None needed for the bootstrap: the user creates the repo, so no API repo creation is required |
| S2 | WowUp import round-trip | Paste a generated string into WowUp-CF on a clean Forever install on macOS and Windows; confirm every addon installs. Also check whether a "missing only" string (a subset) imports cleanly. | Users can still use WowUp's own export on machine A; Forever Sync then only stores that string |
| S3 | WowUp data files | `addons.json` / `preferences.json` path on Windows (`%APPDATA%\WowUpCf`?), plain WowUp vs WowUp-CF differences, schema drift across WowUp versions | If WowUp's data can't be read, addon sync is turned off and settings sync carries on |
| S4 | Forever on Windows | Exe name and path under `_forever_`, default install root, `product.db` entry | Path-prefix process matching already avoids depending on the exe name |
| S5 | libgit2 transport | HTTPS token push on Windows (WinHTTP) and macOS | Bundle MinGit on Windows for transport only |
| S6 | Code signing | Apple notarization; Windows Authenticode (Azure Trusted Signing) | Required before a public beta |

Other risks:
- **Forever is new (launched 2026-11-04):** file layout and TOC conventions may still change. Keeping the facts in one constants module (§3.6) limits the blast radius.
- **Blizzard server-side settings sync** overlaps with ours for bindings, macros and edit mode. The server wins at login, which does no harm.
- **A wrong reconcile on an additional machine:** mitigated by the forced preview, backups and history.

---

## 12. Milestones

| M | Scope | Exit criteria |
|---|---|---|
| M0 | Spikes S1–S4 | Auth model decided; WowUp import round-trip confirmed |
| M1 ✅ (2026-11-18) | Core + CLI: Forever discovery, rules, scanner + Lua check, snapshot into a local bare repo, apply with backups, game-running gate | Live macOS install: 126 files round-trip byte-exact into a fresh install. **Deferred:** stat cache (a full scan takes about 0.5 s at 4 MB), Windows CI run |
| M2 ✅ (2026-11-18) | Remote: device flow, token refresh + Keychain, repo setup + privacy guard, fetch/merge/push, conflict policy, restore | Two CLI machines synced through a real private GitHub repo: first sync, edits, a conflict (newest wins, recorded in the merge commit), deletion, restore; token refresh and Keychain storage verified; integration tests cover the same cases plus unapplied remote changes |
| M3 | Desktop: Tauri shell, onboarding, dashboard, tray, autostart, process and fs watchers | Dogfood on two machines, settings only |
| M4 | Addons: WowUp reader, folder listing, manifest merge, import string, status checklist | The reference install's 23 addons reproduce on a clean machine through one WowUp import |
| M5 | History/restore UI, conflicts UI, signing, auto-update | Public beta |

---

## 13. Open decisions
1. ~~Auth model~~ → **decided:** GitHub App with device flow, under the maintainer's personal account.
2. ~~App name~~ → **decided:** "Forever Sync" (slug `forever-sync`, data repo `forever-sync-data`). Other WoW versions are permanently out of scope.
