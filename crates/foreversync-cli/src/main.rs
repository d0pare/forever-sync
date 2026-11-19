use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use foreversync_addons::{ImportPayload, default_wowup_dirs, import_string, load_inventory};
use foreversync_core::sync::{ApplyOutcome, ConflictPolicy, Engine, FirstSync, Integration, SnapshotOutcome};
use foreversync_core::appdata::{self, RepoConfig};
use foreversync_core::{Changes, ForeverInstall, addon_cleanup, addon_sync, discovery, history, layout, process, scan};
use foreversync_git::{Remote, Store};
use foreversync_github::{self as gh, FileStore, KeychainStore, TokenStore};

/// Forever Sync developer CLI.
///
/// Read-only: detect, addons, status, log, whoami. `snapshot` writes only to
/// the local data repo. `apply`, `restore` and `sync` write into WTF/ (with
/// backups) and won't while the Forever client is running.
#[derive(Parser)]
#[command(name = "foreversync", version)]
struct Cli {
    /// WoW install root (the folder containing `_forever_`). Auto-detected if omitted.
    #[arg(long, global = true)]
    root: Option<PathBuf>,
    /// Forever Sync data folder (repo, backups, config). Default: OS app-data folder.
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,
    /// Keep GitHub tokens in this file instead of the OS keychain (development).
    #[arg(long, global = true, env = "FOREVERSYNC_TOKEN_FILE")]
    token_file: Option<PathBuf>,
    /// Print JSON instead of text (detect, addons, sync).
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Find the WoW Forever install and list accounts, realms and characters.
    Detect,
    /// List addons WowUp manages for Forever, and folders it doesn't.
    Addons {
        /// Print only the WowUp import string (all addons on this machine).
        #[arg(long)]
        import_string: bool,
        /// Print only the WowUp import string for synced addons missing here.
        #[arg(long, conflicts_with = "import_string")]
        missing: bool,
        /// Move addons uninstalled on other computers to the AddOns trash
        /// (shows the plan; add --yes to do it).
        #[arg(long, conflicts_with_all = ["import_string", "missing"])]
        remove_uninstalled: bool,
        /// Confirm --remove-uninstalled.
        #[arg(long, requires = "remove_uninstalled")]
        yes: bool,
        /// WowUp data folder (default: WowUpCf, then WowUp, in the OS config dir).
        #[arg(long)]
        wowup_dir: Option<PathBuf>,
    },
    /// Sign in to GitHub with a device code.
    Login,
    /// Forget the stored GitHub tokens.
    Logout,
    /// Show the signed-in GitHub user.
    Whoami,
    /// Choose the private GitHub repo to sync with.
    Setup {
        /// `owner/name`; auto-detected from repos the app is installed on if omitted.
        #[arg(long)]
        repo: Option<String>,
    },
    /// Fetch, snapshot, merge, push, and apply (if the game isn't running).
    Sync {
        /// On this machine's first sync: whose files win where both exist.
        #[arg(long, value_enum, default_value_t = FirstSyncArg::UseCloud)]
        first_sync: FirstSyncArg,
        /// Who wins when both machines changed the same file.
        #[arg(long, value_enum, default_value_t = PolicyArg::Newest)]
        policy: PolicyArg,
        /// Apply even if a Forever client seems to be running.
        #[arg(long)]
        force: bool,
        /// Make this computer's WowUp addon list the synced list (drops addons
        /// that came only from other computers).
        #[arg(long)]
        addons_source: bool,
    },
    /// Show local WTF/ changes not yet recorded in a snapshot.
    Status,
    /// Record local WTF/ changes as a new commit (local only).
    Snapshot {
        #[arg(long, default_value = "manual")]
        trigger: String,
    },
    /// List recent commits.
    Log {
        #[arg(short = 'n', long, default_value_t = 10)]
        limit: usize,
    },
    /// Write the latest commit into WTF/ (backs up replaced files).
    Apply {
        #[arg(long)]
        force: bool,
    },
    /// Restore WTF/ to an older commit, as a new commit (undoable).
    Restore {
        /// Commit id prefix, `main~2`, …
        rev: String,
        /// Only this addon's SavedVariables (all accounts and characters).
        #[arg(long, conflicts_with = "path")]
        addon: Option<String>,
        /// Only these repo paths (e.g. wtf/Account/X/SavedVariables/Plater.lua); repeatable.
        #[arg(long)]
        path: Vec<String>,
        #[arg(long)]
        force: bool,
    },
    /// Readable history: who synced what, conflicts, restores.
    History {
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: usize,
    },
    /// What one commit changed (settings grouped by addon, addon list changes).
    Show { rev: String },
}

#[derive(Clone, Copy, ValueEnum)]
enum FirstSyncArg {
    UseCloud,
    UploadLocal,
}

#[derive(Clone, Copy, ValueEnum)]
enum PolicyArg {
    Newest,
    Local,
    Remote,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let data_dir = match &cli.data_dir {
        Some(d) => d.clone(),
        None if cfg!(windows) => {
            for item in appdata::migrate_legacy_data_dir()? {
                eprintln!("Moved {item} to the new data folder.");
            }
            appdata::default_data_dir().context("no app-data folder on this OS")?
        }
        None => appdata::default_data_dir().context("no app-data folder on this OS")?,
    };
    let tokens: Box<dyn TokenStore> = match &cli.token_file {
        Some(p) => Box::new(FileStore(p.clone())),
        None => Box::new(KeychainStore::default()),
    };

    // Commands that don't need the game install.
    match &cli.command {
        Command::Login => return login(tokens.as_ref()),
        Command::Logout => {
            tokens.clear()?;
            println!("Signed out.");
            return Ok(());
        }
        Command::Whoami => {
            let user = gh::Client::new(gh::valid_access_token(tokens.as_ref())?).user()?;
            println!("{}", user.login);
            return Ok(());
        }
        Command::Setup { repo } => return setup(tokens.as_ref(), &data_dir, repo.as_deref()),
        Command::Log { limit } => {
            let store = Store::open_or_init(&appdata::repo_dir(&data_dir))?;
            for c in store.log(*limit)? {
                println!("{}  {}  {}", &c.id[..10], format_time(c.time), c.summary);
            }
            return Ok(());
        }
        _ => {}
    }

    let install = discovery::discover(cli.root.as_deref())?;
    let store = Store::open_or_init(&appdata::repo_dir(&data_dir))?;
    let engine = Engine::new(&install, &store, &appdata::backups_dir(&data_dir));

    match cli.command {
        Command::Detect => detect(&install, cli.json),
        Command::Addons { remove_uninstalled: true, yes, wowup_dir, .. } => remove_uninstalled(&install, &store, wowup_dir, yes),
        Command::Addons { import_string, missing, wowup_dir, .. } => {
            addons(&install, &store, cli.json, import_string, missing, wowup_dir)
        }
        Command::Status => {
            let scanned = scan::scan_wtf(&install.wtf_dir())?;
            report_skips(&scanned.skipped);
            let reference = match store.files_at_ref(&engine.disk_ref())? {
                Some(observed) => observed,
                None => layout::wtf_only(&store.head_files()?),
            };
            let changes = Changes::between(&reference, &scanned.files);
            if changes.is_empty() {
                println!("No local changes since the last snapshot ({} files).", scanned.files.len());
            } else {
                print_changes(&changes);
            }
            Ok(())
        }
        Command::Snapshot { trigger } => {
            let out = engine.snapshot(&trigger, FirstSync::UseCloud)?;
            print_snapshot(&out);
            Ok(())
        }
        Command::Apply { force } => {
            print_apply(&engine.apply_head(force)?);
            Ok(())
        }
        Command::Restore { rev, addon, path, force } => {
            let paths = match (&addon, path.is_empty()) {
                (Some(a), _) => {
                    let mut set = std::collections::BTreeSet::new();
                    set.extend(history::addon_settings_paths(&store.files_at(&rev)?, a));
                    set.extend(history::addon_settings_paths(&store.head_files()?, a));
                    if set.is_empty() {
                        bail!("no SavedVariables for addon {a} at {rev} or now");
                    }
                    Some(set.into_iter().collect::<Vec<_>>())
                }
                (None, false) => Some(path),
                (None, true) => None,
            };
            let (commit, applied) = match paths {
                Some(p) => {
                    println!("Restoring {} file(s) from {rev}.", p.len());
                    engine.restore_paths(&rev, &p, force)?
                }
                None => engine.restore(&rev, force)?,
            };
            match commit {
                Some(c) => println!("Restore committed as {}", &c[..10]),
                None => println!("Latest commit already matches {rev}."),
            }
            print_apply(&applied);
            Ok(())
        }
        Command::Sync { first_sync, policy, force, addons_source } => {
            let cfg = appdata::load_config(&data_dir)?.context("no repo configured; run `foreversync setup` first")?;
            let token = gh::valid_access_token(tokens.as_ref())?;
            // Privacy guard (DESIGN.md §8.3): never push to a public repo.
            let info = gh::Client::new(token.clone()).repo(&cfg.repo)?;
            if !info.private {
                bail!("{} is public. Forever Sync only syncs to private repos; make it private on GitHub.", cfg.repo);
            }
            let remote = Remote { url: cfg.clone_url.clone(), token: Some(token) };
            let first = match first_sync {
                FirstSyncArg::UseCloud => FirstSync::UseCloud,
                FirstSyncArg::UploadLocal => FirstSync::UploadLocal,
            };
            let policy = match policy {
                PolicyArg::Newest => ConflictPolicy::NewestWins,
                PolicyArg::Local => ConflictPolicy::PreferLocal,
                PolicyArg::Remote => ConflictPolicy::PreferRemote,
            };
            let mut engine = engine;
            engine.addons_source = addons_source;
            let report = engine.sync(Some(&remote), policy, first, "manual", force)?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&report)?);
                return Ok(());
            }
            print_snapshot(&report.snapshot);
            match &report.integration {
                Integration::FastForward => println!("Took changes from {}.", cfg.repo),
                Integration::Merged { conflicts } => {
                    println!("Merged changes from {}.", cfg.repo);
                    for c in conflicts {
                        println!("  ! conflict {}: kept {:?} version (the other is in history)", c.path, c.kept);
                    }
                }
                Integration::UpToDate | Integration::LocalAhead | Integration::NoRemote => {}
            }
            if let Some(reset) = &report.addon_list_reset {
                println!(
                    "Addon list now matches this computer: {} added, {} updated, {} removed.",
                    reset.added.len(),
                    reset.modified.len(),
                    reset.deleted.len()
                );
                for p in &reset.deleted {
                    println!("  removed from list: {p}");
                }
            }
            if report.pushed {
                println!("Pushed to {}.", cfg.repo);
            }
            print_apply(&report.apply);
            Ok(())
        }
        Command::History { limit } => {
            for e in history::list(&store, limit)? {
                let mut extra = Vec::new();
                if e.settings_changed > 0 {
                    extra.push(format!("{} settings", e.settings_changed));
                }
                if e.addons_changed > 0 {
                    extra.push(format!("{} addons", e.addons_changed));
                }
                if !e.conflicts.is_empty() {
                    extra.push(format!("{} conflict(s)", e.conflicts.len()));
                }
                println!(
                    "{}  {}  {:<14} {:?}/{}  {}",
                    &e.id[..10],
                    format_time(e.time),
                    e.machine.as_deref().unwrap_or("?"),
                    e.kind,
                    e.trigger.as_deref().unwrap_or("-"),
                    extra.join(", ")
                );
            }
            Ok(())
        }
        Command::Show { rev } => {
            let d = history::details(&store, &rev)?;
            println!("{}  {}  {}", &d.entry.id[..10], format_time(d.entry.time), d.entry.summary);
            for c in &d.entry.conflicts {
                println!("  conflict: {} (kept {})", c.path, c.kept);
            }
            let mut last = None;
            for f in &d.settings {
                if last != Some(&f.addon) {
                    println!("  {}:", f.addon.as_deref().unwrap_or("Other settings"));
                    last = Some(&f.addon);
                }
                let who = match (&f.character, &f.account) {
                    (Some(c), _) => format!("char {c}"),
                    (None, Some(a)) => format!("account {a}"),
                    _ => "all accounts".into(),
                };
                println!("    {:<8} {who} · {}", format!("{:?}", f.status).to_lowercase(), f.file);
            }
            for a in &d.addons {
                println!("  addon {:<8} {}", format!("{:?}", a.status).to_lowercase(), a.name);
            }
            Ok(())
        }
        Command::Login | Command::Logout | Command::Whoami | Command::Setup { .. } | Command::Log { .. } => {
            unreachable!("handled above")
        }
    }
}

fn login(tokens: &dyn TokenStore) -> Result<()> {
    let code = gh::start_device_flow()?;
    println!("Open {} and enter code {}", code.verification_uri, code.user_code);
    println!("Waiting for approval (expires in {} min)…", code.expires_in / 60);
    let t = gh::poll_device_flow(&code)?;
    tokens.save(&t)?;
    let user = gh::Client::new(t.access_token).user()?;
    println!("Signed in as {}.", user.login);
    Ok(())
}

fn setup(tokens: &dyn TokenStore, data_dir: &std::path::Path, repo: Option<&str>) -> Result<()> {
    let client = gh::Client::new(gh::valid_access_token(tokens)?);
    let info = match repo {
        Some(r) => client.repo(r).with_context(|| format!("can't access {r}; is Forever Sync installed on it?"))?,
        None => match client.find_data_repo()? {
            Some(d) => {
                println!(
                    "Found {} ({})",
                    d.repo.full_name,
                    if d.initialized { "has Forever Sync data" } else { "empty, will be initialized" }
                );
                d.repo
            }
            None => {
                println!("No data repo found. Create one and give Forever Sync access to it:");
                println!("  1. {}", gh::config::new_repo_url());
                println!("  2. {}  (choose \"Only select repositories\")", gh::config::install_url());
                println!("Then run `foreversync setup` again.");
                return Ok(());
            }
        },
    };
    if !info.private {
        bail!("{} is public. Make it private first: SavedVariables can contain chat and character data.", info.full_name);
    }
    appdata::save_config(data_dir, &RepoConfig { repo: info.full_name.clone(), clone_url: info.clone_url.clone() })?;
    println!("This machine will sync with {} (private). Run `foreversync sync`.", info.full_name);
    Ok(())
}

fn print_snapshot(s: &SnapshotOutcome) {
    report_skips(&s.skipped);
    for p in &s.overrode_pending {
        println!("  ! local edit replaced an incoming change: {p}");
    }
    match &s.commit {
        Some(c) => {
            print_changes(&s.changes);
            println!("Snapshot {}.", &c[..10]);
        }
        None => println!("No local changes."),
    }
}

fn print_apply(a: &ApplyOutcome) {
    match a {
        ApplyOutcome::UpToDate => println!("WTF/ is up to date."),
        ApplyOutcome::GameRunning => println!("WoW Forever is running: changes will be applied after you quit."),
        ApplyOutcome::Applied(r) => {
            println!("Applied to WTF/: {} written, {} deleted.", r.written, r.deleted);
            if let Some(b) = &r.backup_dir {
                println!("Replaced files backed up to {}", b.display());
            }
        }
    }
}

fn detect(install: &ForeverInstall, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(install)?);
        return Ok(());
    }
    println!("WoW Forever install: {}", install.root.display());
    println!("Client version:      {}", install.version.as_deref().unwrap_or("unknown"));
    println!("Client running:      {}", if process::forever_running(&install.flavor_dir) { "yes" } else { "no" });
    for acct in &install.accounts {
        println!("Account {}", acct.name);
        for realm in &acct.realms {
            let chars: Vec<_> = realm.characters.iter().map(|c| c.folder.as_str()).collect();
            println!("  realm {:<6} {}", realm.id, chars.join(", "));
        }
    }
    Ok(())
}

fn addons(
    install: &ForeverInstall,
    store: &Store,
    json: bool,
    only_string: bool,
    only_missing: bool,
    wowup_dir: Option<PathBuf>,
) -> Result<()> {
    let dirs = wowup_dir.map(|d| vec![d]).unwrap_or_else(default_wowup_dirs);
    let inv = load_inventory(&dirs, &install.flavor_dir, &install.addons_dir())?;
    let now_ms = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
    let payload = ImportPayload::from_addons(&inv.managed, now_ms);
    let string = import_string(&payload);
    let synced = store.head_files()?;
    let status = (!addon_sync::packages_in(&synced).is_empty()).then(|| addon_sync::status(&synced, &inv));

    if only_missing {
        match status.as_ref().and_then(|s| s.missing_import.clone()) {
            Some(s) => println!("{s}"),
            None => eprintln!("Nothing missing: every synced addon is installed here."),
        }
    } else if only_string {
        println!("{string}");
    } else if json {
        let out = serde_json::json!({ "inventory": inv, "import_payload": payload, "import_string": string, "status": status });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!("WowUp data: {}", inv.wowup_dir.display());
        println!("\nManaged by WowUp ({}), synced as a list:", inv.managed.len());
        for a in &inv.managed {
            println!(
                "  {:<44} {:<9} {:<9} {}",
                truncate(&a.name, 44),
                a.provider,
                a.external_id,
                a.version.as_deref().unwrap_or("-")
            );
        }
        println!("\nNot managed by WowUp ({}), not synced:", inv.unmanaged_folders.len());
        for f in &inv.unmanaged_folders {
            println!("  {f}");
        }
        if inv.dropped_records > 0 {
            println!("\n({} stale or duplicate WowUp records ignored)", inv.dropped_records);
        }
        match &status {
            None => println!("\nNo synced addon list yet (run `foreversync sync`)."),
            Some(st) => {
                println!("\nSynced list: {} installed here", st.in_sync);
                if !st.missing.is_empty() {
                    println!("Missing here ({}), install with `foreversync addons --missing` → WowUp import:", st.missing.len());
                    for p in &st.missing {
                        println!("  {}", p.name);
                    }
                }
                if !st.removed_elsewhere.is_empty() {
                    println!("Uninstalled on another machine ({}), remove in WowUp if unwanted:", st.removed_elsewhere.len());
                    for p in &st.removed_elsewhere {
                        println!("  {}", p.name);
                    }
                }
            }
        }
        println!("\nWowUp import string ({} chars): run with --import-string to print it", string.len());
    }
    Ok(())
}

fn remove_uninstalled(install: &ForeverInstall, store: &Store, wowup_dir: Option<PathBuf>, yes: bool) -> Result<()> {
    let dirs = wowup_dir.map(|d| vec![d]).unwrap_or_else(default_wowup_dirs);
    let inv = load_inventory(&dirs, &install.flavor_dir, &install.addons_dir())?;
    let plan = addon_cleanup::plan(&store.head_files()?, &inv, None);
    if plan.packages.is_empty() {
        println!("Nothing to remove: every addon here is in the synced list.");
        return Ok(());
    }
    println!("Uninstalled on another computer ({}):", plan.packages.len());
    for p in &plan.packages {
        println!("  {}", p.name);
    }
    println!("Folders to move to {}:", install.addons_dir().with_file_name(addon_cleanup::TRASH_DIR).display());
    for f in &plan.folders {
        println!("  {f}");
    }
    for f in &plan.kept_shared {
        println!("  (kept, still used by another addon: {f})");
    }
    if !yes {
        println!("Run again with --yes to move them.");
        return Ok(());
    }
    if process::forever_running(&install.flavor_dir) {
        bail!("Quit WoW Forever first; addons can't be removed while the game is running.");
    }
    let r = addon_cleanup::execute(&install.addons_dir(), &plan)?;
    println!("Moved {} folder(s).", r.moved.len());
    if let Some(t) = r.trash_dir {
        println!("They're in {} (emptied after 30 days); move them back to restore.", t.display());
    }
    Ok(())
}

fn report_skips(skipped: &[scan::Skipped]) {
    for s in skipped {
        println!("  ! skipped {}: {}", s.path, s.reason);
    }
}

fn print_changes(c: &Changes) {
    for p in &c.added {
        println!("  added    {p}");
    }
    for p in &c.modified {
        println!("  modified {p}");
    }
    for p in &c.deleted {
        println!("  deleted  {p}");
    }
}

fn format_time(unix: i64) -> String {
    let secs = unix.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(unix.div_euclid(86_400));
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02} UTC", secs / 3600, (secs % 3600) / 60)
}

/// Howard Hinnant's days→civil algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        let mut t: String = s.chars().take(max - 1).collect();
        t.push('…');
        t
    }
}
