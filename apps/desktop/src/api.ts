import { invoke } from "@tauri-apps/api/core";

export type Character = { folder: string };
export type Realm = { id: string; characters: Character[] };
export type Account = { name: string; realms: Realm[] };
export type ForeverInstall = { root: string; flavor_dir: string; version: string | null; accounts: Account[] };

export type LastSync = {
  at: number;
  trigger: string;
  local_changes: number;
  took_remote: boolean;
  conflicts: string[];
  pushed: boolean;
  written: number;
  deleted: number;
  pending_apply: boolean;
  skipped: string[];
  addons_missing: number;
  addons_removed_elsewhere: number;
  addon_changes: number;
};
export type Status = { syncing: boolean; game_running: boolean; last_sync: LastSync | null; last_error: string | null };
export type RepoConfig = { repo: string; clone_url: string };
export type AppStateView = {
  signed_in: boolean;
  user: string | null;
  repo: RepoConfig | null;
  onboarded: boolean;
  autostart: boolean;
  install: ForeverInstall | null;
  install_error: string | null;
  status: Status;
  data_dir: string;
  version: string;
  update: UpdateInfo | null;
};
export type UpdateInfo = { current: string; version: string; notes: string | null };
export type LoginStart = { user_code: string; verification_uri: string; expires_in: number };
export type RepoChoice = { full_name: string; private: boolean; initialized: boolean };
export type RepoSearch = { found: RepoChoice | null; new_repo_url: string; install_url: string };
export type ManagedAddon = { name: string; provider: string; external_id: string; version: string | null; folders: string[] };
export type Inventory = { wowup_dir: string; managed: ManagedAddon[]; unmanaged_folders: string[] };
export type PackageFile = { name: string; provider: string; id: string; version: string | null; folders: string[] };
export type AddonStatus = {
  missing: PackageFile[];
  removed_elsewhere: PackageFile[];
  in_sync: number;
  missing_import: string | null;
};
export type RemovalPlan = { packages: PackageFile[]; folders: string[]; kept_shared: string[] };
export type RemovalReport = { moved: string[]; trash_dir: string | null };
export type AddonsView = {
  inventory: Inventory | null;
  error: string | null;
  import_string: string | null;
  status: AddonStatus | null;
  synced_count: number;
};

export type HistoryKind = "snapshot" | "merge" | "restore" | "addonssource" | "other";
export type ConflictNote = { path: string; kept: string };
export type HistoryEntry = {
  id: string;
  time: number;
  summary: string;
  kind: HistoryKind;
  machine: string | null;
  trigger: string | null;
  settings_changed: number;
  addons_changed: number;
  conflicts: ConflictNote[];
  parents: string[];
};
export type ChangeStatus = "added" | "modified" | "deleted";
export type SettingsFile = {
  path: string;
  status: ChangeStatus;
  file: string;
  account: string | null;
  realm: string | null;
  character: string | null;
  addon: string | null;
};
export type HistoryDetails = {
  entry: HistoryEntry;
  settings: SettingsFile[];
  addons: { name: string; status: ChangeStatus; version: string | null }[];
};
export type Effect = { this_computer: SettingsFile[]; cloud: SettingsFile[] };
export type FirstSyncPreview = {
  cloud_has_data: boolean;
  synced_before: boolean;
  use_cloud: Effect;
  upload_local: Effect;
  same: number;
  addons_missing: number | null;
};
export type RestoreScope =
  | { kind: "all" }
  | { kind: "addon"; addon: string }
  | { kind: "paths"; paths: string[] }
  | { kind: "other"; path: string; kept: string };
export type RestoreResult = {
  commit: string | null;
  files: number;
  applied: "applied" | "game-running" | "up-to-date";
  backup_dir: string | null;
  pushed: boolean;
  sync_error: string | null;
};

export const api = {
  history: (limit?: number) => invoke<HistoryEntry[]>("history", { limit }),
  historyDetails: (id: string) => invoke<HistoryDetails>("history_details", { id }),
  restore: (rev: string, scope: RestoreScope) => invoke<RestoreResult>("restore", { rev, scope }),
  appState: () => invoke<AppStateView>("app_state"),
  startLogin: () => invoke<LoginStart>("start_login"),
  finishLogin: () => invoke<string>("finish_login"),
  logout: () => invoke<void>("logout"),
  findRepo: () => invoke<RepoSearch>("find_repo"),
  chooseRepo: (fullName: string) => invoke<RepoChoice>("choose_repo", { fullName }),
  firstSyncPreview: () => invoke<FirstSyncPreview>("first_sync_preview"),
  syncNow: (firstSync?: "use-cloud" | "upload-local") => invoke<LastSync | null>("sync_now", { firstSync }),
  addonRemovalPlan: () => invoke<RemovalPlan>("addon_removal_plan"),
  removeAddons: () => invoke<RemovalReport>("remove_addons"),
  checkUpdate: () => invoke<UpdateInfo | null>("check_update"),
  installUpdate: () => invoke<void>("install_update"),
  makeAddonSource: () => invoke<LastSync | null>("sync_now", { addonsSource: true }),
  setAutostart: (enabled: boolean) => invoke<boolean>("set_autostart", { enabled }),
  addons: () => invoke<AddonsView>("addons"),
};
