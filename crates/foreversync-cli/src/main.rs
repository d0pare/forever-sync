mod state;

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use foreversync_addons::{ImportPayload, default_wowup_dirs, import_string, load_inventory};
use foreversync_core::{Changes, ForeverInstall, apply, discovery, layout, process, scan};
use foreversync_git::Store;

/// Forever Sync developer CLI.
///
/// `detect`, `addons`, `status` and `log` are read-only. `snapshot` writes only
/// to the local data repo. `apply` writes into WTF/ (with backups) and refuses
/// while the Forever client is running.
#[derive(Parser)]
#[command(name = "foreversync", version)]
struct Cli {
    /// WoW install root (the folder containing `_forever_`). Auto-detected if omitted.
    #[arg(long, global = true)]
    root: Option<PathBuf>,
    /// Forever Sync data folder (repo, backups, state). Default: OS app-data folder.
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,
    /// Print JSON instead of text (detect, addons).
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
        /// Print only the WowUp import string.
        #[arg(long)]
        import_string: bool,
        /// WowUp data folder (default: WowUpCf, then WowUp, in the OS config dir).
        #[arg(long)]
        wowup_dir: Option<PathBuf>,
    },
    /// Show how WTF/ on disk differs from the latest snapshot.
    Status,
    /// Record WTF/ as a new commit in the local data repo (if anything changed).
    Snapshot {
        /// What triggered the snapshot (recorded in the commit).
        #[arg(long, default_value = "manual")]
        trigger: String,
    },
    /// List recent snapshots.
    Log {
        #[arg(short = 'n', long, default_value_t = 10)]
        limit: usize,
    },
    /// Write a snapshot back into WTF/, backing up every file it replaces.
    Apply {
        /// Commit to apply (id, `main`, `main~1`, …).
        #[arg(long, default_value = "main")]
        rev: String,
        /// Only show what would change.
        #[arg(long)]
        dry_run: bool,
        /// Apply even if a Forever client seems to be running (it will overwrite the result on exit).
        #[arg(long)]
        force: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let install = discovery::discover(cli.root.as_deref())?;
    let data_dir = match &cli.data_dir {
        Some(d) => d.clone(),
        None => dirs::data_local_dir().context("no app-data folder on this OS")?.join("Forever Sync"),
    };
    let repo_dir = data_dir.join("repo.git");

    match cli.command {
        Command::Detect => detect(&install, cli.json),
        Command::Addons { import_string, wowup_dir } => addons(&install, cli.json, import_string, wowup_dir),
        Command::Status => {
            let store = Store::open_or_init(&repo_dir)?;
            let scanned = scan::scan_wtf(&install.wtf_dir())?;
            report_skips(&scanned);
            let changes = Changes::between(&layout::wtf_only(&store.head_files()?), &scanned.files);
            if store.head()?.is_none() {
                println!("No snapshots yet; {} files would be recorded.", scanned.files.len());
            } else if changes.is_empty() {
                println!("WTF/ matches the latest snapshot ({} files).", scanned.files.len());
            } else {
                print_changes(&changes);
            }
            Ok(())
        }
        Command::Snapshot { trigger } => {
            let store = Store::open_or_init(&repo_dir)?;
            let scanned = scan::scan_wtf(&install.wtf_dir())?;
            report_skips(&scanned);
            let before = layout::wtf_only(&store.head_files()?);
            let mut files = layout::base_files();
            files.extend(scanned.files.clone());

            let host = process::host_name();
            let message = format!(
                "Snapshot from \"{host}\" ({trigger})\n\n\
                 ForeverSync-Machine: {host}\n\
                 ForeverSync-Trigger: {trigger}\n\
                 ForeverSync-Client: wowf {}\n\
                 ForeverSync-Version: {}\n",
                install.version.as_deref().unwrap_or("unknown"),
                env!("CARGO_PKG_VERSION"),
            );
            match store.snapshot(&files, &message)? {
                Some(id) => {
                    print_changes(&Changes::between(&before, &scanned.files));
                    println!("Committed {} ({} files) to {}", &id.to_string()[..10], scanned.files.len(), repo_dir.display());
                }
                None => println!("No changes since the last snapshot ({} files).", scanned.files.len()),
            }
            let head = store.head()?.expect("repo has a commit after snapshot");
            state::set_base(&data_dir, &install, &head.to_string())?;
            Ok(())
        }
        Command::Log { limit } => {
            let store = Store::open_or_init(&repo_dir)?;
            for c in store.log(limit)? {
                println!("{}  {}  {}", &c.id[..10], format_time(c.time), c.summary);
            }
            Ok(())
        }
        Command::Apply { rev, dry_run, force } => {
            let store = Store::open_or_init(&repo_dir)?;
            if store.head()?.is_none() {
                bail!("no snapshots in {}", repo_dir.display());
            }
            if !dry_run && !force && process::forever_running(&install.flavor_dir) {
                bail!("WoW Forever is running from {}. Quit the game first (it overwrites WTF/ on exit).", install.flavor_dir.display());
            }
            let rev_id = store.resolve(&rev)?;
            let target = store.files_at(&rev_id)?;
            let base = match state::base(&data_dir, &install)? {
                Some(id) => Some(store.files_at(&id)?),
                None => None,
            };
            let plan = apply::plan(&install.wtf_dir(), &target, base.as_ref())?;
            for p in &plan.refused {
                println!("  ! not allowed by sync rules, ignored: {p}");
            }
            if plan.is_empty() {
                println!("Nothing to apply: WTF/ already matches {rev}.");
                return Ok(());
            }
            for p in &plan.writes {
                println!("  write  {p}");
            }
            for p in &plan.deletes {
                println!("  delete {p}");
            }
            if dry_run {
                println!("Dry run: {} writes, {} deletes. Nothing changed.", plan.writes.len(), plan.deletes.len());
                return Ok(());
            }
            let report = apply::execute(&install.wtf_dir(), &target, &plan, &data_dir.join("backups"))?;
            println!("Applied {rev}: {} written, {} deleted.", report.written, report.deleted);
            if let Some(b) = report.backup_dir {
                println!("Replaced files backed up to {}", b.display());
            }
            state::set_base(&data_dir, &install, &rev_id)?;
            Ok(())
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

fn addons(install: &ForeverInstall, json: bool, only_string: bool, wowup_dir: Option<PathBuf>) -> Result<()> {
    let dirs = wowup_dir.map(|d| vec![d]).unwrap_or_else(default_wowup_dirs);
    let inv = load_inventory(&dirs, &install.flavor_dir, &install.addons_dir())?;
    let now_ms = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
    let payload = ImportPayload::from_addons(&inv.managed, now_ms);
    let string = import_string(&payload);

    if only_string {
        println!("{string}");
    } else if json {
        let out = serde_json::json!({ "inventory": inv, "import_payload": payload, "import_string": string });
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
        println!("\nWowUp import string ({} chars): run with --import-string to print it", string.len());
    }
    Ok(())
}

fn report_skips(scanned: &scan::ScanResult) {
    for s in &scanned.skipped {
        println!("  ! skipped {}: {}", s.path, s.reason);
    }
    for (p, size) in &scanned.large {
        println!("  ! large file ({:.1} MB): {p}", *size as f64 / 1_048_576.0);
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
    // Days/hours since epoch without a date crate: good enough for a dev CLI.
    let secs = unix.rem_euclid(86_400);
    let days = unix.div_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
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
