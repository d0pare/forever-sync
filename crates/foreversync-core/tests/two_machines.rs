//! Two fake machines syncing through a local bare remote (DESIGN.md §12 M2).

use std::fs;
use std::path::{Path, PathBuf};

use foreversync_core::discovery;
use foreversync_core::merge::Side;
use foreversync_core::sync::{ApplyOutcome, ConflictPolicy, Engine, FirstSync, Integration};
use foreversync_git::{Remote, Store};

struct Machine {
    root: PathBuf,
    data: PathBuf,
}

impl Machine {
    fn new(base: &Path, name: &str) -> Self {
        let root = base.join(name).join("World of Warcraft");
        fs::create_dir_all(root.join("_forever_/WTF/Account/ACCT/SavedVariables")).unwrap();
        fs::write(root.join("_forever_/.flavor.info"), "Product Flavor!STRING:0\nwowf\n").unwrap();
        Self { root, data: base.join(name).join("data") }
    }
    fn wtf(&self, rel: &str) -> PathBuf {
        self.root.join("_forever_/WTF").join(rel)
    }
    fn write(&self, rel: &str, s: &str) {
        let p = self.wtf(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, s).unwrap();
    }
    /// This machine's fake WowUp data folder, if `set_wowup` was called.
    fn wowup_dirs(&self) -> Vec<PathBuf> {
        let d = self.data.parent().unwrap().join("WowUpCf");
        if d.exists() { vec![d] } else { Vec::new() }
    }
    /// Writes fake WowUp data listing `addons` (name, id, version) and creates
    /// their folders under Interface/AddOns.
    fn set_wowup(&self, addons: &[(&str, &str, &str)]) {
        let d = self.data.parent().unwrap().join("WowUpCf");
        fs::create_dir_all(&d).unwrap();
        let flavor = self.root.join("_forever_");
        fs::write(
            d.join("preferences.json"),
            serde_json::json!({ "wow_installations": [
                { "id": "inst", "clientType": 10, "location": flavor.join("Wow.exe") }
            ]}).to_string(),
        )
        .unwrap();
        let addons_dir = flavor.join("Interface/AddOns");
        let _ = fs::remove_dir_all(&addons_dir);
        let mut records = serde_json::Map::new();
        for (name, id, ver) in addons {
            fs::create_dir_all(addons_dir.join(name)).unwrap();
            records.insert(
                id.to_string(),
                serde_json::json!({
                    "id": id, "name": name, "installationId": "inst", "providerName": "Curse",
                    "externalId": id, "installedExternalReleaseId": format!("{id}{ver}"),
                    "installedVersion": ver, "installedFolders": name, "isIgnored": false,
                    "installedAt": "2026-11-18T00:00:00Z"
                }),
            );
        }
        fs::write(d.join("addons.json"), serde_json::Value::Object(records).to_string()).unwrap();
    }
    fn sync_as_addon_source(&self, remote: &Remote) -> foreversync_core::sync::SyncReport {
        let install = discovery::open(&self.root).unwrap();
        let store = Store::open_or_init(&self.data.join("repo.git")).unwrap();
        let mut engine = Engine::new(&install, &store, &self.data.join("backups"));
        engine.wowup_dirs = self.wowup_dirs();
        engine.addons_source = true;
        engine.sync(Some(remote), ConflictPolicy::NewestWins, FirstSync::UseCloud, "test", false).unwrap()
    }
    fn addon_status(&self) -> foreversync_core::addon_sync::AddonStatus {
        let store = Store::open_or_init(&self.data.join("repo.git")).unwrap();
        let install = discovery::open(&self.root).unwrap();
        let inv = foreversync_addons::load_inventory(&self.wowup_dirs(), &install.flavor_dir, &install.addons_dir()).unwrap();
        foreversync_core::addon_sync::status(&store.head_files().unwrap(), &inv)
    }
    fn read(&self, rel: &str) -> Option<String> {
        fs::read_to_string(self.wtf(rel)).ok()
    }
    fn sync(&self, remote: &Remote, policy: ConflictPolicy, first: FirstSync) -> foreversync_core::sync::SyncReport {
        let install = discovery::open(&self.root).unwrap();
        let store = Store::open_or_init(&self.data.join("repo.git")).unwrap();
        let mut engine = Engine::new(&install, &store, &self.data.join("backups"));
        engine.wowup_dirs = self.wowup_dirs();
        engine.sync(Some(remote), policy, first, "test", false).unwrap()
    }
}

const SV: &str = "Account/ACCT/SavedVariables";

fn lua(name: &str, v: &str) -> String {
    format!("{name} = {{\n[\"v\"] = \"{v}\",\n}}\n")
}

#[test]
fn two_machines_through_a_remote() {
    let tmp = tempfile::tempdir().unwrap();
    let origin = tmp.path().join("origin.git");
    git2::Repository::init_bare(&origin).unwrap();
    let remote = Remote { url: origin.display().to_string(), token: None };
    let a = Machine::new(tmp.path(), "a");
    let b = Machine::new(tmp.path(), "b");
    let nw = ConflictPolicy::NewestWins;

    // 1. A uploads. B's first sync uses the cloud for overlaps, keeps B-only files.
    a.write(&format!("{SV}/Plater.lua"), &lua("PlaterDB", "a1"));
    a.write("Account/ACCT/93/Alpha-One/AddOns.txt", "Plater: enabled\n");
    a.write("Config.wtf", "SET gxApi \"metal\"\n");
    let r = a.sync(&remote, nw, FirstSync::UseCloud);
    assert!(r.pushed);

    b.write(&format!("{SV}/Plater.lua"), &lua("PlaterDB", "b-stale"));
    b.write(&format!("{SV}/OnlyB.lua"), &lua("OnlyB", "b"));
    let r = b.sync(&remote, nw, FirstSync::UseCloud);
    assert!(r.pushed, "B's own file goes up");
    assert_eq!(b.read(&format!("{SV}/Plater.lua")).unwrap(), lua("PlaterDB", "a1"), "cloud wins on first sync");
    assert_eq!(b.read("Account/ACCT/93/Alpha-One/AddOns.txt").unwrap(), "Plater: enabled\n");
    assert_eq!(b.read("Config.wtf"), None, "Config.wtf never syncs");
    let ApplyOutcome::Applied(rep) = &r.apply else { panic!("B should apply: {:?}", r.apply) };
    let backup = rep.backup_dir.as_ref().expect("B's stale Plater.lua was backed up");
    assert_eq!(fs::read_to_string(backup.join(format!("{SV}/Plater.lua"))).unwrap(), lua("PlaterDB", "b-stale"));

    a.sync(&remote, nw, FirstSync::UseCloud);
    assert_eq!(a.read(&format!("{SV}/OnlyB.lua")).unwrap(), lua("OnlyB", "b"));

    // 2. A edits; B picks it up.
    a.write(&format!("{SV}/Plater.lua"), &lua("PlaterDB", "a2"));
    a.sync(&remote, nw, FirstSync::UseCloud);
    let r = b.sync(&remote, nw, FirstSync::UseCloud);
    assert_eq!(r.integration, Integration::FastForward);
    assert_eq!(b.read(&format!("{SV}/Plater.lua")).unwrap(), lua("PlaterDB", "a2"));

    // 3. Both edit the same file offline: newest snapshot wins, A converges.
    a.write(&format!("{SV}/Plater.lua"), &lua("PlaterDB", "a3"));
    a.sync(&remote, nw, FirstSync::UseCloud);
    std::thread::sleep(std::time::Duration::from_millis(1100)); // commit times have 1 s resolution
    b.write(&format!("{SV}/Plater.lua"), &lua("PlaterDB", "b3"));
    b.write(&format!("{SV}/OnlyB.lua"), &lua("OnlyB", "b3"));
    let r = b.sync(&remote, nw, FirstSync::UseCloud);
    let Integration::Merged { conflicts } = &r.integration else { panic!("expected merge: {:?}", r.integration) };
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].path, format!("wtf/{SV}/Plater.lua"));
    assert_eq!(conflicts[0].kept, Side::Local, "B's snapshot is newer");
    a.sync(&remote, nw, FirstSync::UseCloud);
    assert_eq!(a.read(&format!("{SV}/Plater.lua")).unwrap(), lua("PlaterDB", "b3"));
    assert_eq!(a.read(&format!("{SV}/OnlyB.lua")).unwrap(), lua("OnlyB", "b3"));

    // 4. A deletes a file; B's copy goes (backed up). B never "deletes" a
    //    character it never had on disk.
    fs::remove_file(a.wtf(&format!("{SV}/OnlyB.lua"))).unwrap();
    a.write("Account/ACCT/93/Beta-Two/AddOns.txt", "Questie: enabled\n");
    a.sync(&remote, nw, FirstSync::UseCloud);
    b.sync(&remote, nw, FirstSync::UseCloud);
    assert_eq!(b.read(&format!("{SV}/OnlyB.lua")), None);
    assert_eq!(b.read("Account/ACCT/93/Beta-Two/AddOns.txt").unwrap(), "Questie: enabled\n");
    a.sync(&remote, nw, FirstSync::UseCloud);
    assert_eq!(a.read("Account/ACCT/93/Beta-Two/AddOns.txt").unwrap(), "Questie: enabled\n");
}

#[test]
fn unapplied_remote_change_survives_local_snapshot() {
    let tmp = tempfile::tempdir().unwrap();
    let origin = tmp.path().join("origin.git");
    git2::Repository::init_bare(&origin).unwrap();
    let remote = Remote { url: origin.display().to_string(), token: None };
    let a = Machine::new(tmp.path(), "a");
    let b = Machine::new(tmp.path(), "b");
    let nw = ConflictPolicy::NewestWins;

    a.write(&format!("{SV}/X.lua"), &lua("X", "1"));
    a.write(&format!("{SV}/Y.lua"), &lua("Y", "1"));
    a.sync(&remote, nw, FirstSync::UseCloud);
    b.sync(&remote, nw, FirstSync::UseCloud);

    a.write(&format!("{SV}/X.lua"), &lua("X", "2"));
    a.sync(&remote, nw, FirstSync::UseCloud);

    // B is "in game": it fetches and integrates but cannot apply.
    let install = discovery::open(&b.root).unwrap();
    let store = Store::open_or_init(&b.data.join("repo.git")).unwrap();
    let mut engine = Engine::new(&install, &store, &b.data.join("backups"));
    engine.wowup_dirs = Vec::new();
    let theirs = store.fetch(&remote).unwrap();
    engine.integrate(theirs, nw).unwrap();
    // While playing, B changes Y (e.g. /reload) and snapshots.
    b.write(&format!("{SV}/Y.lua"), &lua("Y", "b"));
    engine.snapshot("watch", FirstSync::UseCloud).unwrap();
    let head = store.head_files().unwrap();
    assert_eq!(head[&format!("wtf/{SV}/X.lua")], lua("X", "2").into_bytes(), "remote X not reverted");
    assert_eq!(head[&format!("wtf/{SV}/Y.lua")], lua("Y", "b").into_bytes());
    assert_eq!(b.read(&format!("{SV}/X.lua")).unwrap(), lua("X", "1"), "disk untouched while in game");

    // After the game exits, apply brings X in and keeps B's Y.
    let ApplyOutcome::Applied(_) = engine.apply_head(false).unwrap() else { panic!("should apply") };
    assert_eq!(b.read(&format!("{SV}/X.lua")).unwrap(), lua("X", "2"));
    assert_eq!(b.read(&format!("{SV}/Y.lua")).unwrap(), lua("Y", "b"));
}

#[test]
fn addon_lists_merge_and_report_missing_and_removed() {
    let tmp = tempfile::tempdir().unwrap();
    let origin = tmp.path().join("origin.git");
    git2::Repository::init_bare(&origin).unwrap();
    let remote = Remote { url: origin.display().to_string(), token: None };
    let pc = Machine::new(tmp.path(), "pc");
    let mac = Machine::new(tmp.path(), "mac");
    let nw = ConflictPolicy::NewestWins;
    let names = |v: &[foreversync_core::addon_sync::PackageFile]| v.iter().map(|p| p.name.clone()).collect::<Vec<_>>();

    pc.set_wowup(&[("Plater", "1", "v1"), ("Questie", "2", "12"), ("RestedXP", "3", "4")]);
    mac.set_wowup(&[("Plater", "1", "v1"), ("OnlyMac", "9", "1")]);
    let r = pc.sync(&remote, nw, FirstSync::UseCloud);
    assert_eq!(r.snapshot.addon_changes.added.len(), 3);
    mac.sync(&remote, nw, FirstSync::UseCloud);
    pc.sync(&remote, nw, FirstSync::UseCloud);

    // The synced list is the union; each machine sees what it lacks.
    let s = mac.addon_status();
    assert_eq!(names(&s.missing), ["Questie", "RestedXP"]);
    assert!(s.missing_import.is_some());
    assert_eq!(names(&pc.addon_status().missing), ["OnlyMac"]);

    // Mac installs the missing ones (newer Questie): nothing missing anymore.
    mac.set_wowup(&[("Plater", "1", "v1"), ("OnlyMac", "9", "1"), ("Questie", "2", "13"), ("RestedXP", "3", "4")]);
    mac.sync(&remote, nw, FirstSync::UseCloud);
    assert!(mac.addon_status().missing.is_empty());

    // PC uninstalls RestedXP: removed from the list, Mac is told to remove it.
    pc.set_wowup(&[("Plater", "1", "v1"), ("Questie", "2", "12"), ("OnlyMac", "9", "1")]);
    let r = pc.sync(&remote, nw, FirstSync::UseCloud);
    assert_eq!(r.snapshot.addon_changes.deleted, ["addons/packages/curse-3.json"]);
    mac.sync(&remote, nw, FirstSync::UseCloud);
    let s = mac.addon_status();
    assert_eq!(names(&s.removed_elsewhere), ["RestedXP"]);
    assert!(s.missing.is_empty());

    // A machine whose WowUp can't be read never drops addons from the list.
    fs::remove_file(mac.data.parent().unwrap().join("WowUpCf/addons.json")).unwrap();
    let r = mac.sync(&remote, nw, FirstSync::UseCloud);
    assert!(r.snapshot.addons_error.is_some());
    assert!(r.snapshot.addon_changes.is_empty(), "unreadable WowUp must not delete: {:?}", r.snapshot.addon_changes);
}

#[test]
fn one_machine_can_become_the_addon_source() {
    let tmp = tempfile::tempdir().unwrap();
    let origin = tmp.path().join("origin.git");
    git2::Repository::init_bare(&origin).unwrap();
    let remote = Remote { url: origin.display().to_string(), token: None };
    let pc = Machine::new(tmp.path(), "pc");
    let mac = Machine::new(tmp.path(), "mac");
    let nw = ConflictPolicy::NewestWins;
    let names = |v: &[foreversync_core::addon_sync::PackageFile]| v.iter().map(|p| p.name.clone()).collect::<Vec<_>>();

    // The Mac syncs its list first (it has an addon the PC doesn't want).
    mac.set_wowup(&[("Plater", "1", "v1"), ("MacExtra", "9", "1")]);
    mac.sync(&remote, nw, FirstSync::UseCloud);
    pc.set_wowup(&[("Plater", "1", "v1"), ("Questie", "2", "12"), ("RestedXP", "3", "4")]);

    // PC makes its list the source: MacExtra leaves the synced list.
    let r = pc.sync_as_addon_source(&remote);
    let reset = r.addon_list_reset.expect("reset ran");
    assert_eq!(reset.deleted, ["addons/packages/curse-9.json"]);
    assert!(pc.addon_status().missing.is_empty(), "PC's list is exactly the synced list");

    // The Mac is told what to install and what to remove; its next syncs
    // don't put MacExtra back.
    mac.sync(&remote, nw, FirstSync::UseCloud);
    let s = mac.addon_status();
    assert_eq!(names(&s.missing), ["Questie", "RestedXP"]);
    assert_eq!(names(&s.removed_elsewhere), ["MacExtra"]);
    mac.sync(&remote, nw, FirstSync::UseCloud);
    pc.sync(&remote, nw, FirstSync::UseCloud);
    assert!(pc.addon_status().missing.is_empty(), "MacExtra stayed out of the list");

    // Refuses instead of wiping the list when WowUp can't be read.
    fs::remove_file(pc.data.parent().unwrap().join("WowUpCf/addons.json")).unwrap();
    let install = discovery::open(&pc.root).unwrap();
    let store = Store::open_or_init(&pc.data.join("repo.git")).unwrap();
    let mut engine = Engine::new(&install, &store, &pc.data.join("backups"));
    engine.wowup_dirs = pc.wowup_dirs();
    engine.addons_source = true;
    assert!(engine.sync(Some(&remote), nw, FirstSync::UseCloud, "test", false).is_err());
    assert_eq!(foreversync_core::addon_sync::packages_in(&store.head_files().unwrap()).len(), 3);
}
