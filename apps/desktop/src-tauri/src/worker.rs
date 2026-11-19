//! Background syncing (DESIGN.md §7.1): after the game exits, and every
//! 10 minutes. Syncing while the game runs only records and pushes local
//! changes; incoming changes are applied after the game exits.

use std::time::{Duration, Instant};

use foreversync_core::sync::FirstSync;
use foreversync_core::{ForeverInstall, discovery, process};
use tauri::{AppHandle, Manager};

use crate::state::{AppState, DesktopSettings};
use crate::syncer::{run_sync, update_status};

const TICK: Duration = Duration::from_secs(5);
const PERIOD: Duration = Duration::from_secs(10 * 60);
/// Let WoW finish writing SavedVariables after the process exits.
const SETTLE: Duration = Duration::from_secs(3);
const STARTUP_DELAY: Duration = Duration::from_secs(10);

pub fn spawn(app: AppHandle) {
    std::thread::Builder::new()
        .name("forever-sync-worker".into())
        .spawn(move || run(app))
        .expect("spawn worker thread");
}

fn run(app: AppHandle) {
    let mut install: Option<ForeverInstall> = None;
    let mut was_running = false;
    // First background sync shortly after startup.
    let mut next_periodic = Instant::now() + STARTUP_DELAY;

    loop {
        std::thread::sleep(TICK);
        let data_dir = app.state::<AppState>().data_dir.clone();
        if !DesktopSettings::load(&data_dir).onboarded {
            continue;
        }
        if install.is_none() {
            install = discovery::discover(None).ok();
        }
        let running = install.as_ref().is_some_and(|i| process::forever_running(&i.flavor_dir));
        if running != app.state::<AppState>().status.lock().unwrap().game_running {
            update_status(&app, |s| s.game_running = running);
        }

        if was_running && !running {
            std::thread::sleep(SETTLE);
            let _ = run_sync(&app, "game-exit", FirstSync::UseCloud);
            next_periodic = Instant::now() + PERIOD;
        } else if Instant::now() >= next_periodic {
            let trigger = if running { "while-playing" } else { "schedule" };
            if run_sync(&app, trigger, FirstSync::UseCloud).is_err() {
                install = None; // rediscover next time (install may have moved)
            }
            next_periodic = Instant::now() + PERIOD;
        }
        was_running = running;
    }
}
