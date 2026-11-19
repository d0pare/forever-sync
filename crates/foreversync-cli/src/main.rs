use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use clap::{Parser, Subcommand};
use foreversync_addons::{ImportPayload, default_wowup_dirs, import_string, load_inventory};
use foreversync_core::discovery;

/// Forever Sync developer CLI. Every command here is read-only.
#[derive(Parser)]
#[command(name = "foreversync", version)]
struct Cli {
    /// WoW install root (the folder containing `_forever_`). Auto-detected if omitted.
    #[arg(long, global = true)]
    root: Option<PathBuf>,
    /// Print JSON instead of text.
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
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let install = discovery::discover(cli.root.as_deref())?;

    match cli.command {
        Command::Detect => {
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&install)?);
                return Ok(());
            }
            println!("WoW Forever install: {}", install.root.display());
            println!("Client version:      {}", install.version.as_deref().unwrap_or("unknown"));
            for acct in &install.accounts {
                println!("Account {}", acct.name);
                for realm in &acct.realms {
                    let chars: Vec<_> = realm.characters.iter().map(|c| c.folder.as_str()).collect();
                    println!("  realm {:<6} {}", realm.id, chars.join(", "));
                }
            }
        }
        Command::Addons { import_string: only_string, wowup_dir } => {
            let dirs = wowup_dir.map(|d| vec![d]).unwrap_or_else(default_wowup_dirs);
            let inv = load_inventory(&dirs, &install.flavor_dir, &install.addons_dir())?;
            let now_ms = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
            let payload = ImportPayload::from_addons(&inv.managed, now_ms);
            let string = import_string(&payload);

            if only_string {
                println!("{string}");
            } else if cli.json {
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
        }
    }
    Ok(())
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
