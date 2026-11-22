import { useCallback, useEffect, useState } from "react";
import { plural } from "./util";
import { api, ConflictNote, HistoryDetails, HistoryEntry, RestoreResult, RestoreScope, SettingsFile } from "./api";

const TRIGGERS: Record<string, string> = {
  manual: "Sync now",
  "game-exit": "after quitting WoW",
  schedule: "scheduled",
  "while-playing": "while playing",
  restore: "restore",
  "addons-source": "addon list set",
  "keep-addons": "kept addons",
};

function title(e: HistoryEntry) {
  switch (e.kind) {
    case "merge":
      return "Combined changes from both computers";
    case "restore":
      return "Restored settings";
    case "addonssource":
      return "Addon list set from this computer";
    case "snapshot":
      return `Snapshot (${TRIGGERS[e.trigger ?? ""] ?? e.trigger ?? "sync"})`;
    default:
      return e.summary;
  }
}

function when(t: number) {
  const d = new Date(t * 1000);
  const today = new Date();
  const sameDay = d.toDateString() === today.toDateString();
  return sameDay
    ? d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })
    : d.toLocaleString([], { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });
}


export function History({ onRestored }: { onRestored: () => void }) {
  const [entries, setEntries] = useState<HistoryEntry[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [open, setOpen] = useState<string | null>(null);

  const load = useCallback(() => {
    api.history(50).then(setEntries).catch((e) => setError(String(e)));
  }, []);
  useEffect(load, [load]);

  if (error) return <section><p className="error">{error}</p></section>;
  if (!entries) return <section><p className="muted">Loading history…</p></section>;
  if (!entries.length) return <section><p className="muted">No snapshots yet.</p></section>;

  return (
    <section>
      <div className="row">
        <h2>History</h2>
        <button className="link" onClick={load}>
          Refresh
        </button>
      </div>
      <p className="muted small">
        Every sync is a snapshot. Restoring makes a new snapshot, so you can always undo it, and it reaches your other
        computers on their next sync.
      </p>
      <ul className="history">
        {entries.map((e) => (
          <li key={e.id} className={open === e.id ? "open" : ""}>
            <button className="entry" onClick={() => setOpen(open === e.id ? null : e.id)}>
              <span className="when">{when(e.time)}</span>
              <span className="what">
                {title(e)}
                {e.conflicts.length > 0 && <span className="badge warn-badge">{plural(e.conflicts.length, "conflict")}</span>}
              </span>
              <span className="muted small">
                {e.machine ?? "?"}
                {e.settings_changed > 0 && ` · ${plural(e.settings_changed, "settings file")}`}
                {e.addons_changed > 0 && ` · ${plural(e.addons_changed, "addon")}`}
              </span>
            </button>
            {open === e.id && (
              <Details
                id={e.id}
                onRestored={() => {
                  load();
                  onRestored();
                }}
              />
            )}
          </li>
        ))}
      </ul>
    </section>
  );
}

function Details({ id, onRestored }: { id: string; onRestored: () => void }) {
  const [d, setD] = useState<HistoryDetails | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    api.historyDetails(id).then(setD).catch((e) => setError(String(e)));
  }, [id]);
  if (error) return <p className="error">{error}</p>;
  if (!d) return <p className="muted small">Loading…</p>;

  // Group settings files by addon; files without an addon go under "Other settings".
  const groups = new Map<string, SettingsFile[]>();
  for (const f of d.settings) {
    const k = f.addon ?? "";
    groups.set(k, [...(groups.get(k) ?? []), f]);
  }
  const added = d.addons.filter((a) => a.status === "added");
  const removed = d.addons.filter((a) => a.status === "deleted");
  const updated = d.addons.filter((a) => a.status === "modified");

  return (
    <div className="details">
      {d.entry.conflicts.length > 0 && (
        <div className="warn">
          <p>Changed on both computers; one version was kept:</p>
          {d.entry.conflicts.map((c) => (
            <ConflictRow key={c.path} rev={d.entry.id} conflict={c} onRestored={onRestored} />
          ))}
        </div>
      )}

      {d.settings.length > 0 && (
        <>
          <RestoreButton
            rev={d.entry.id}
            scope={{ kind: "all" }}
            label="Restore all settings to this point"
            confirm="Every synced settings file goes back to how it was in this snapshot. Your current files are backed up first."
            onRestored={onRestored}
          />
          {[...groups.entries()].map(([addon, files]) => (
            <div key={addon || "_other"} className="group">
              <div className="row">
                <strong>{addon || "Other settings"}</strong>
                {addon && (
                  <RestoreButton
                    rev={d.entry.id}
                    scope={{ kind: "addon", addon }}
                    label={`Restore ${addon}’s settings from here`}
                    confirm={`${addon}’s SavedVariables on every account and character go back to this snapshot. Nothing else changes.`}
                    onRestored={onRestored}
                    small
                  />
                )}
              </div>
              <ul className="files">
                {files.map((f) => (
                  <li key={f.path}>
                    <span className={`st st-${f.status}`}>{f.status}</span> {where(f)}
                    {!addon && (
                      <RestoreButton
                        rev={d.entry.id}
                        scope={{ kind: "paths", paths: [f.path] }}
                        label="restore"
                        confirm={`Restore ${f.file} (${where(f)}) to this snapshot?`}
                        onRestored={onRestored}
                        small
                        link
                      />
                    )}
                  </li>
                ))}
              </ul>
            </div>
          ))}
        </>
      )}

      {d.addons.length > 0 && (
        <p className="small">
          {added.length > 0 && <>Addons added: {added.map((a) => a.name).join(", ")}. </>}
          {removed.length > 0 && <>Removed: {removed.map((a) => a.name).join(", ")}. </>}
          {updated.length > 0 && <>Updated: {updated.map((a) => `${a.name}${a.version ? ` (${a.version})` : ""}`).join(", ")}.</>}
        </p>
      )}
      {!d.settings.length && !d.addons.length && !d.entry.conflicts.length && (
        <p className="muted small">No settings or addon changes in this snapshot.</p>
      )}
    </div>
  );
}

export function where(f: SettingsFile) {
  const who = f.character ? `${f.character} (realm ${f.realm})` : f.account ? `account ${f.account}` : "all accounts";
  return `${who} · ${f.file}`;
}

function ConflictRow({ rev, conflict, onRestored }: { rev: string; conflict: ConflictNote; onRestored: () => void }) {
  const file = conflict.path.replace(/^wtf\//, "");
  const keptWho = conflict.kept === "local" ? "the computer that combined them" : "the other computer";
  return (
    <div className="row small">
      <span>
        <code>{file}</code> <span className="muted">(kept the version from {keptWho})</span>
      </span>
      <RestoreButton
        rev={rev}
        scope={{ kind: "other", path: conflict.path, kept: conflict.kept }}
        label="Use the other version"
        confirm="Switch this file to the version that wasn't kept? The current one stays in history."
        onRestored={onRestored}
        small
      />
    </div>
  );
}

function RestoreButton(props: {
  rev: string;
  scope: RestoreScope;
  label: string;
  confirm: string;
  onRestored: () => void;
  small?: boolean;
  link?: boolean;
}) {
  const [asking, setAsking] = useState(false);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const go = async () => {
    setBusy(true);
    setError(null);
    try {
      const r: RestoreResult = await api.restore(props.rev, props.scope);
      setResult(describe(r));
      setAsking(false);
      props.onRestored();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  if (result) return <span className="ok-text small"> {result}</span>;
  if (!asking)
    return (
      <button className={props.link ? "link small" : props.small ? "small" : ""} onClick={() => setAsking(true)}>
        {props.label}
      </button>
    );
  return (
    <div className="confirm">
      <p className="small">{props.confirm}</p>
      <div className="row">
        <button onClick={() => setAsking(false)} disabled={busy}>
          Cancel
        </button>
        <button className="primary" onClick={go} disabled={busy}>
          {busy ? "Restoring…" : "Restore"}
        </button>
      </div>
      {error && <p className="error small">{error}</p>}
    </div>
  );
}

function describe(r: RestoreResult) {
  if (!r.commit) return "Already matches this snapshot.";
  const where =
    r.applied === "applied"
      ? "Restored in your WTF folder (previous files backed up)"
      : r.applied === "game-running"
        ? "Restored; applies when you quit WoW"
        : "Restored";
  const shared = r.pushed ? " and sent to your other computers." : r.sync_error ? `; sync failed: ${r.sync_error}` : ".";
  return where + shared;
}
