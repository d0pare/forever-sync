//! Is the Forever client running? (DESIGN.md §3.3)
//!
//! Matched by executable **path**: the Forever `.app` has the same name as
//! the Retail one, and the Windows exe name is unverified. Any process whose
//! executable lives under `_forever_/` counts.

use std::path::Path;

use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

/// Returns `true` when the process list can't be read, so callers err on the
/// side of not writing.
pub fn forever_running(flavor_dir: &Path) -> bool {
    let flavor_dir = flavor_dir.canonicalize().unwrap_or_else(|_| flavor_dir.to_path_buf());
    let Ok(mut sys) = System::new() else { return true };
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_exe(UpdateKind::OnlyIfNotSet),
    );
    sys.processes()
        .values()
        .filter_map(|p| p.exe())
        .any(|exe| exe.starts_with(&flavor_dir))
}

/// This machine's host name, for commit trailers.
pub fn host_name() -> String {
    System::host_name().unwrap_or_else(|_| "unknown-host".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_dir_has_no_client() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!forever_running(tmp.path()));
    }
}
