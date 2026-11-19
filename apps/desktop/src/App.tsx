import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  api,
  AddonsView,
  AppStateView,
  Effect,
  FirstSyncPreview,
  LastSync,
  LoginStart,
  RemovalPlan,
  RepoChoice,
  RepoSearch,
  SettingsFile,
  Status,
  UpdateInfo,
} from "./api";

function useCopy() {
  const [copied, setCopied] = useState<string | null>(null);
  const copy = async (key: string, text: string) => {
    await navigator.clipboard.writeText(text);
    setCopied(key);
    setTimeout(() => setCopied(null), 1500);
  };
  return { copied, copy };
}
import { History, where } from "./History";
import "./App.css";

export default function App() {
  const [state, setState] = useState<AppStateView | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(() => {
    api.appState().then(setState).catch((e) => setError(String(e)));
  }, []);

  useEffect(() => {
    refresh();
    const un = listen<Status>("status", (e) => setState((s) => (s ? { ...s, status: e.payload } : s)));
    const un2 = listen<UpdateInfo | null>("update", (e) => setState((s) => (s ? { ...s, update: e.payload } : s)));
    return () => {
      un.then((f) => f());
      un2.then((f) => f());
    };
  }, [refresh]);

  if (error) return <Shell><p className="error">{error}</p></Shell>;
  if (!state) return <Shell><p className="muted">Loading…</p></Shell>;
  if (!state.signed_in) return <Shell><SignIn onDone={refresh} /></Shell>;
  if (!state.repo) return <Shell><ChooseRepo onDone={refresh} /></Shell>;
  if (!state.onboarded) return <Shell><FirstSync state={state} onDone={refresh} /></Shell>;
  return <Shell><Dashboard state={state} onChange={refresh} /></Shell>;
}

function Shell({ children }: { children: React.ReactNode }) {
  return (
    <main>
      <header>
        <h1>Forever Sync</h1>
      </header>
      {children}
    </main>
  );
}

// ---------------------------------------------------------------- onboarding

function SignIn({ onDone }: { onDone: () => void }) {
  const [code, setCode] = useState<LoginStart | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const start = async () => {
    setError(null);
    setBusy(true);
    try {
      const c = await api.startLogin();
      setCode(c);
      await openUrl(c.verification_uri);
      await api.finishLogin();
      onDone();
    } catch (e) {
      setError(String(e));
      setCode(null);
    } finally {
      setBusy(false);
    }
  };

  return (
    <section>
      <Step n={1} of={3} title="Sign in with GitHub" />
      <p>
        Forever Sync keeps your WoW Forever settings in a private GitHub repository that you own. Sign in to
        connect it.
      </p>
      {code ? (
        <div className="code-box">
          <p className="muted">Enter this code on GitHub:</p>
          <p className="code">{code.user_code}</p>
          <div className="row">
            <button onClick={() => navigator.clipboard.writeText(code.user_code)}>Copy code</button>
            <button onClick={() => openUrl(code.verification_uri)}>Open GitHub</button>
          </div>
          <p className="muted">Waiting for you to approve…</p>
        </div>
      ) : (
        <button className="primary" onClick={start} disabled={busy}>
          Sign in with GitHub
        </button>
      )}
      {error && <p className="error">{error}</p>}
    </section>
  );
}

function ChooseRepo({ onDone }: { onDone: () => void }) {
  const [search, setSearch] = useState<RepoSearch | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const look = useCallback(async () => {
    setError(null);
    setBusy(true);
    try {
      setSearch(await api.findRepo());
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }, []);
  useEffect(() => {
    look();
  }, [look]);

  const choose = async (r: RepoChoice) => {
    setError(null);
    try {
      await api.chooseRepo(r.full_name);
      onDone();
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <section>
      <Step n={2} of={3} title="Choose where to keep your settings" />
      {busy && <p className="muted">Looking for your Forever Sync repository…</p>}
      {search?.found && (
        <>
          <p>
            Found <strong>{search.found.full_name}</strong>{" "}
            <span className="muted">
              ({search.found.initialized ? "already has settings from another computer" : "empty, ready to use"})
            </span>
          </p>
          <button className="primary" onClick={() => choose(search.found!)}>
            Use {search.found.full_name}
          </button>
        </>
      )}
      {search && !search.found && (
        <>
          <p>Create a private repository for your settings and give Forever Sync access to it:</p>
          <ol>
            <li>
              <a onClick={() => openUrl(search.new_repo_url)}>Create the repository</a>{" "}
              <span className="muted">(name and “Private” are filled in; click “Create repository”)</span>
            </li>
            <li>
              <a onClick={() => openUrl(search.install_url)}>Give Forever Sync access</a>{" "}
              <span className="muted">(choose “Only select repositories”, then the new repository)</span>
            </li>
          </ol>
          <button className="primary" onClick={look} disabled={busy}>
            I’ve done that, check again
          </button>
        </>
      )}
      {error && <p className="error">{error}</p>}
    </section>
  );
}

function FirstSync({ state, onDone }: { state: AppStateView; onDone: () => void }) {
  const [preview, setPreview] = useState<FirstSyncPreview | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [choice, setChoice] = useState<"use-cloud" | "upload-local">("use-cloud");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!state.install) return;
    setPreview(null);
    setPreviewError(null);
    api
      .firstSyncPreview()
      .then(setPreview)
      .catch((e) => setPreviewError(String(e)));
  }, [state.repo?.repo, state.install?.flavor_dir]);

  // Without a preview, assume the cloud has data so the choice is offered.
  const initialized = preview ? preview.cloud_has_data : previewError ? true : null;
  const effect = preview && (choice === "use-cloud" ? preview.use_cloud : preview.upload_local);

  const go = async () => {
    setError(null);
    setBusy(true);
    try {
      await api.syncNow(initialized ? choice : "upload-local");
      onDone();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section>
      <Step n={3} of={3} title="First sync" />
      {state.install ? (
        <p>
          WoW Forever found at <code>{state.install.root}</code>{" "}
          <span className="muted">(client {state.install.version ?? "unknown"})</span>
        </p>
      ) : (
        <p className="error">{state.install_error}</p>
      )}
      {state.install && initialized === null && <p className="muted">Comparing this computer with {state.repo?.repo}…</p>}
      {previewError && <p className="muted small">Couldn’t compare with the cloud first: {previewError}</p>}
      {initialized === false && (
        <p>
          {preview ? plural(preview.use_cloud.cloud.length, "settings file") : "Your settings"} from this computer will be
          uploaded to <strong>{state.repo?.repo}</strong>. Nothing in your game folder changes.
        </p>
      )}
      {initialized && (
        <>
          <div className="choices">
            <label className={choice === "use-cloud" ? "choice selected" : "choice"}>
              <input type="radio" checked={choice === "use-cloud"} onChange={() => setChoice("use-cloud")} />
              <div>
                <strong>Use the settings from my other computer</strong> <span className="badge">Recommended</span>
                <p className="muted">
                  Where both have a file, the cloud version wins. Files only on this computer are kept. Every replaced
                  file is backed up first.
                </p>
                {preview && <EffectSummary effect={preview.use_cloud} />}
              </div>
            </label>
            <label className={choice === "upload-local" ? "choice selected" : "choice"}>
              <input type="radio" checked={choice === "upload-local"} onChange={() => setChoice("upload-local")} />
              <div>
                <strong>Use this computer’s settings</strong>
                <p className="muted">This computer’s files win and are sent to your other computers.</p>
                {preview && <EffectSummary effect={preview.upload_local} />}
              </div>
            </label>
          </div>
          {preview && preview.same > 0 && (
            <p className="muted small">{plural(preview.same, "file")} already match and won’t change.</p>
          )}
          {effect && (effect.this_computer.length > 0 || effect.cloud.length > 0) && (
            <details className="preview">
              <summary>Show the files</summary>
              <PreviewFiles title="On this computer" files={effect.this_computer} side="here" />
              <PreviewFiles title="Sent to your other computers" files={effect.cloud} side="cloud" />
            </details>
          )}
          {preview && (preview.addons_missing ?? 0) > 0 && (
            <p className="small">
              {plural(preview.addons_missing!, "addon")} from your other computer {preview.addons_missing === 1 ? "isn’t" : "aren’t"}{" "}
              installed here. After syncing you’ll get a WowUp import string for {preview.addons_missing === 1 ? "it" : "them"}.
            </p>
          )}
        </>
      )}
      {state.status.game_running && (
        <p className="warn">WoW Forever is running. Settings are uploaded now; incoming ones are applied after you quit.</p>
      )}
      <button className="primary" onClick={go} disabled={busy || !state.install || initialized === null}>
        {busy ? "Syncing…" : "Start syncing"}
      </button>
      {error && <p className="error">{error}</p>}
    </section>
  );
}

function plural(n: number, w: string) {
  return `${n} ${w}${n === 1 ? "" : "s"}`;
}

/** "On this computer: 3 replaced, 2 new · Sent: 1 new" */
function EffectSummary({ effect }: { effect: Effect }) {
  const count = (files: SettingsFile[], words: Record<SettingsFile["status"], string>) => {
    const parts = (["modified", "added", "deleted"] as const)
      .map((st) => [files.filter((f) => f.status === st).length, words[st]] as const)
      .filter(([n]) => n > 0)
      .map(([n, w]) => `${n} ${w}`);
    return parts.length ? parts.join(", ") : "no changes";
  };
  return (
    <p className="small effect">
      This computer: {count(effect.this_computer, { modified: "replaced", added: "new", deleted: "removed" })}
      <br />
      Other computers: {count(effect.cloud, { modified: "replaced", added: "new", deleted: "removed" })}
    </p>
  );
}

function PreviewFiles({ title, files, side }: { title: string; files: SettingsFile[]; side: "here" | "cloud" }) {
  if (!files.length) return null;
  const label = { modified: "replace", added: "new", deleted: "remove" } as const;
  return (
    <div className="group">
      <strong>{title}</strong>
      <ul className="files">
        {files.map((f) => (
          <li key={f.path} title={f.path.replace(/^wtf\//, "")}>
            <span className={`st st-${f.status}`}>{label[f.status]}</span> {where(f)}
            {side === "here" && f.status === "modified" && <span className="muted">(backed up first)</span>}
          </li>
        ))}
      </ul>
    </div>
  );
}

function Step({ n, of, title }: { n: number; of: number; title: string }) {
  return (
    <>
      <p className="muted step">
        Step {n} of {of}
      </p>
      <h2>{title}</h2>
    </>
  );
}

// ---------------------------------------------------------------- dashboard

function Dashboard({ state, onChange }: { state: AppStateView; onChange: () => void }) {
  const [tab, setTab] = useState<"status" | "history">("status");
  return (
    <>
      <nav className="tabs">
        <button className={tab === "status" ? "tab active" : "tab"} onClick={() => setTab("status")}>
          Status
        </button>
        <button className={tab === "history" ? "tab active" : "tab"} onClick={() => setTab("history")}>
          History
        </button>
      </nav>
      {tab === "status" ? <StatusView state={state} onChange={onChange} /> : <History onRestored={onChange} />}
    </>
  );
}

function StatusView({ state, onChange }: { state: AppStateView; onChange: () => void }) {
  const s = state.status;
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const syncNow = async () => {
    setError(null);
    setBusy(true);
    try {
      await api.syncNow();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      {state.update && <UpdateBanner update={state.update} />}
      <section>
        <div className="row">
          <StatusPill status={s} />
          <button className="primary" onClick={syncNow} disabled={busy || s.syncing}>
            {s.syncing ? "Syncing…" : "Sync now"}
          </button>
        </div>
        {s.last_error && <p className="error">{s.last_error}</p>}
        {error && error !== s.last_error && <p className="error">{error}</p>}
        {s.last_sync ? <LastSyncView last={s.last_sync} /> : <p className="muted">Not synced yet in this session.</p>}
        <p className="muted small">
          Syncs automatically when you quit WoW and every 10 minutes. {state.install && <>Game: {state.install.root}</>}
        </p>
      </section>
      <Addons lastSyncAt={s.last_sync?.at ?? 0} />
      <Settings state={state} onChange={onChange} />
    </>
  );
}

function UpdateBanner({ update }: { update: UpdateInfo }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const install = async () => {
    setBusy(true);
    setError(null);
    try {
      await api.installUpdate(); // restarts the app on success
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  };
  return (
    <section className="callout">
      <div className="row">
        <strong>
          Forever Sync {update.version} is available <span className="muted">(you have {update.current})</span>
        </strong>
        <button className="primary" onClick={install} disabled={busy}>
          {busy ? "Updating…" : "Install and restart"}
        </button>
      </div>
      {update.notes && <p className="muted small pre">{update.notes}</p>}
      {error && <p className="error">{error}</p>}
    </section>
  );
}

function StatusPill({ status: s }: { status: Status }) {
  let text = "Up to date";
  let cls = "pill ok";
  if (s.syncing) [text, cls] = ["Syncing…", "pill busy"];
  else if (s.last_error) [text, cls] = ["Sync problem", "pill bad"];
  else if (s.last_sync?.pending_apply) [text, cls] = ["Changes wait until you quit WoW", "pill warn"];
  else if (!s.last_sync) [text, cls] = ["Waiting for first sync", "pill"];
  return (
    <span className={cls}>
      {text}
      {s.game_running && !s.syncing && " · WoW running"}
    </span>
  );
}

function LastSyncView({ last }: { last: LastSync }) {
  const when = new Date(last.at * 1000).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  const parts: string[] = [];
  if (last.local_changes) parts.push(`${last.local_changes} local change${last.local_changes === 1 ? "" : "s"} uploaded`);
  if (last.took_remote) parts.push("changes from another computer received");
  if (last.written || last.deleted) parts.push(`${last.written + last.deleted} file${last.written + last.deleted === 1 ? "" : "s"} updated in WTF`);
  if (last.addon_changes) parts.push(`addon list updated`);
  if (!parts.length) parts.push("nothing new");
  return (
    <>
      <p>
        Last sync at {when} ({triggerLabel(last.trigger)}): {parts.join(", ")}.
      </p>
      {last.conflicts.length > 0 && (
        <div className="warn">
          <p>Changed on both computers; the newer version was kept (the other is in history):</p>
          <ul>
            {last.conflicts.map((c) => (
              <li key={c}>
                <code>{c.replace(/^wtf\//, "")}</code>
              </li>
            ))}
          </ul>
        </div>
      )}
      {last.skipped.length > 0 && (
        <div className="warn">
          <p>Skipped (not valid Lua, probably mid-write; will retry):</p>
          <ul>
            {last.skipped.map((c) => (
              <li key={c}>
                <code>{c.replace(/^wtf\//, "")}</code>
              </li>
            ))}
          </ul>
        </div>
      )}
    </>
  );
}

function triggerLabel(t: string) {
  return (
    {
      manual: "manual",
      "game-exit": "after quitting WoW",
      schedule: "scheduled",
      "while-playing": "while playing",
      "addons-source": "addon list set from this computer",
    }[t] ?? t
  );
}

function Addons({ lastSyncAt }: { lastSyncAt: number }) {
  const [view, setView] = useState<AddonsView | null>(null);
  const [reload, setReload] = useState(0);
  const { copied, copy } = useCopy();
  useEffect(() => {
    api.addons().then(setView).catch((e) =>
      setView({ inventory: null, error: String(e), import_string: null, status: null, synced_count: 0 }),
    );
  }, [lastSyncAt, reload]); // refresh after every sync and after removals
  const inv = view?.inventory;
  const st = view?.status;
  return (
    <section>
      <h2>Addons (via WowUp)</h2>
      {!view && <p className="muted">Reading WowUp…</p>}
      {view?.error && <p className="error">{view.error}</p>}

      {st && st.missing.length > 0 && (
        <div className="callout">
          <div className="row">
            <strong>
              {st.missing.length} addon{st.missing.length === 1 ? "" : "s"} from your other computer{" "}
              {st.missing.length === 1 ? "isn’t" : "aren’t"} installed here
            </strong>
            <button className="primary" onClick={() => copy("missing", st.missing_import!)}>
              {copied === "missing" ? "Copied" : "Copy WowUp import for these"}
            </button>
          </div>
          <p className="muted small">{st.missing.map((p) => p.name).join(", ")}</p>
          <p className="muted small">
            In WowUp: select WoW Forever → My Addons → Import/Export Addons → paste → Install. This list updates
            after the next sync.
          </p>
        </div>
      )}
      {st && st.removed_elsewhere.length > 0 && (
        <div className="warn">
          <p>
            Uninstalled on your other computer (WowUp’s import doesn’t remove addons):
          </p>
          <p className="small">{st.removed_elsewhere.map((p) => p.name).join(", ")}</p>
          <RemoveAddons count={st.removed_elsewhere.length} onDone={() => setReload((n) => n + 1)} />
        </div>
      )}
      {st && st.missing.length === 0 && st.removed_elsewhere.length === 0 && (
        <p className="ok-text">All {st.in_sync} synced addons are installed here.</p>
      )}
      {!st && inv && (
        <p className="muted small">The addon list is shared with your other computers after the next sync.</p>
      )}

      {inv && (
        <>
          <details>
            <summary>{inv.managed.length} addons installed here (WowUp)</summary>
            <table>
              <tbody>
                {inv.managed.map((a) => (
                  <tr key={`${a.provider}:${a.external_id}`}>
                    <td>{a.name}</td>
                    <td className="muted">{a.version ?? "–"}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </details>
          {inv.unmanaged_folders.length > 0 && (
            <p className="muted small">Not managed by WowUp (not synced): {inv.unmanaged_folders.join(", ")}</p>
          )}
          <p className="small">
            <button className="link" onClick={() => copy("all", view!.import_string!)} disabled={!view?.import_string}>
              {copied === "all" ? "Copied" : "Copy WowUp import for all addons on this computer"}
            </button>
          </p>
          <MakeAddonSource count={inv.managed.length} />
        </>
      )}
    </section>
  );
}

function RemoveAddons({ count, onDone }: { count: number; onDone: () => void }) {
  const [plan, setPlan] = useState<RemovalPlan | null>(null);
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const ask = async () => {
    setError(null);
    try {
      setPlan(await api.addonRemovalPlan());
    } catch (e) {
      setError(String(e));
    }
  };
  const go = async () => {
    setBusy(true);
    setError(null);
    try {
      const r = await api.removeAddons();
      setMsg(`Moved ${r.moved.length} folder${r.moved.length === 1 ? "" : "s"} to the trash${r.trash_dir ? ` (${r.trash_dir})` : ""}. Kept for 30 days; move a folder back to restore it.`);
      setPlan(null);
      onDone();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };
  if (msg) return <p className="small">{msg}</p>;
  if (!plan)
    return (
      <>
        <button onClick={ask}>Remove {count === 1 ? "it" : `these ${count}`} from this computer…</button>
        {error && <p className="error">{error}</p>}
      </>
    );
  return (
    <div className="callout">
      <p className="small">
        These AddOns folders will be moved to a trash folder next to AddOns (WoW ignores it) and kept for 30 days:
      </p>
      <p className="small">
        <code>{plan.folders.join(", ") || "none"}</code>
      </p>
      {plan.kept_shared.length > 0 && (
        <p className="muted small">Kept because another addon still uses them: {plan.kept_shared.join(", ")}</p>
      )}
      <p className="muted small">WowUp will notice they’re gone the next time it scans. Quit WoW first.</p>
      <div className="row">
        <button onClick={() => setPlan(null)} disabled={busy}>
          Cancel
        </button>
        <button className="primary" onClick={go} disabled={busy || plan.folders.length === 0}>
          {busy ? "Removing…" : `Move ${plan.folders.length} folder${plan.folders.length === 1 ? "" : "s"} to trash`}
        </button>
      </div>
      {error && <p className="error">{error}</p>}
    </div>
  );
}

function VersionLine({ version }: { version: string }) {
  const [msg, setMsg] = useState<string | null>(null);
  const check = async () => {
    setMsg("Checking…");
    try {
      const u = await api.checkUpdate();
      setMsg(u ? `Version ${u.version} is available (see the banner above).` : "You have the latest version.");
    } catch (e) {
      setMsg(`Couldn't check for updates: ${e}`);
    }
  };
  return (
    <p className="muted small">
      Forever Sync {version} ·{" "}
      <button className="link" onClick={check}>
        Check for updates
      </button>
      {msg && <> · {msg}</>}
    </p>
  );
}

function MakeAddonSource({ count }: { count: number }) {
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);
  const go = async () => {
    setBusy(true);
    setMsg(null);
    try {
      await api.makeAddonSource();
      setMsg("Done. Your other computers will show what to install or remove after their next sync.");
    } catch (e) {
      setMsg(String(e));
    } finally {
      setBusy(false);
      setConfirming(false);
    }
  };
  if (!confirming)
    return (
      <p className="small">
        <button className="link" onClick={() => setConfirming(true)}>
          Make this computer’s addons the list to follow…
        </button>
        {msg && <span className="muted"> {msg}</span>}
      </p>
    );
  return (
    <div className="callout">
      <p className="small">
        The synced addon list will become exactly the {count} addons installed here. Addons that came only from your
        other computers are dropped from the list, and those computers will be told to remove them in WowUp. No addon
        files are deleted anywhere.
      </p>
      <div className="row">
        <button onClick={() => setConfirming(false)} disabled={busy}>
          Cancel
        </button>
        <button className="primary" onClick={go} disabled={busy}>
          {busy ? "Syncing…" : "Use this computer’s addon list"}
        </button>
      </div>
    </div>
  );
}

function Settings({ state, onChange }: { state: AppStateView; onChange: () => void }) {
  const [autostart, setAutostart] = useState(state.autostart);
  return (
    <section>
      <h2>Settings</h2>
      <label className="toggle">
        <input
          type="checkbox"
          checked={autostart}
          onChange={async (e) => setAutostart(await api.setAutostart(e.target.checked))}
        />
        Start Forever Sync when I log in
      </label>
      <p className="small">
        Repository:{" "}
        <a onClick={() => openUrl(`https://github.com/${state.repo?.repo}`)}>{state.repo?.repo}</a>
        <span className="muted"> (private)</span>
      </p>
      <p className="small">
        Signed in as <strong>{state.user ?? "…"}</strong>{" "}
        <button
          className="link"
          onClick={async () => {
            await api.logout();
            onChange();
          }}
        >
          Sign out
        </button>
      </p>
      <p className="muted small">Backups and local history: {state.data_dir}</p>
      <VersionLine version={state.version} />
    </section>
  );
}
