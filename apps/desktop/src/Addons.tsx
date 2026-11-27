import { useEffect, useMemo, useState } from "react";
import { api, AddonKey, AddonRow, AddonState, AddonsView, PackageFile, RemovalPlan, RemovalReport } from "./api";
import { plural, useCopy } from "./util";

const STATE: Record<AddonState, { label: string; cls: string; hint: string }> = {
  in_sync: { label: "Synced", cls: "ok", hint: "Installed here and on the synced list." },
  missing: { label: "To install", cls: "accent", hint: "On the synced list but not installed here." },
  removed_elsewhere: { label: "Removed elsewhere", cls: "warn", hint: "Uninstalled on another computer." },
  new_here: { label: "New here", cls: "accent", hint: "Installed since the last sync; added to the list at the next one." },
  removed_here: { label: "Uninstalled here", cls: "muted", hint: "Dropped from the list at the next sync." },
  updating: { label: "Updating", cls: "muted", hint: "WowUp lists it but its folders are missing right now." },
};

const keyOf = (p: { provider: string; id: string }): AddonKey => ({ provider: p.provider, id: p.id });
const same = (a: AddonKey, b: AddonKey) => a.provider === b.provider && a.id === b.id;

export function Addons({ lastSyncAt, onChange }: { lastSyncAt: number; onChange: () => void }) {
  const [view, setView] = useState<AddonsView | null>(null);
  const [reload, setReload] = useState(0);
  const [notice, setNotice] = useState<string | null>(null);
  const refresh = (msg?: string) => {
    setNotice(msg ?? null);
    setReload((n) => n + 1);
    onChange();
  };
  useEffect(() => {
    api
      .addons()
      .then(setView)
      .catch((e) =>
        setView({
          error: String(e),
          wowup_dir: null,
          rows: [],
          status: null,
          unmanaged_folders: [],
          import_string: null,
          synced_count: 0,
          trash: null,
          game_running: false,
        }),
      );
  }, [lastSyncAt, reload]);

  if (!view) return <section><p className="muted">Reading WowUp…</p></section>;
  const st = view.status;
  return (
    <>
      {view.error && (
        <section>
          <h2>Addons</h2>
          <p className="error">{view.error}</p>
          <p className="muted small">
            Forever Sync reads the addon list from WowUp (WowUp-CF). Install WowUp and add your WoW Forever folder to
            it, then come back here.
          </p>
        </section>
      )}
      {st && <Summary view={view} />}
      {notice && (
        <p className="notice small" onClick={() => setNotice(null)}>
          {notice}
        </p>
      )}
      {st && st.missing.length > 0 && <ToInstall missing={st.missing} importString={st.missing_import!} />}
      {st && st.removed_elsewhere.length > 0 && (
        <RemovedElsewhere addons={st.removed_elsewhere} gameRunning={view.game_running} onDone={refresh} />
      )}
      {st && st.wowup_leftovers.length > 0 && st.removed_elsewhere.length === 0 && (
        <Leftovers addons={st.wowup_leftovers} onDone={refresh} />
      )}
      {st && (st.new_here.length > 0 || st.removed_here.length > 0) && <Pending status={st} onDone={refresh} />}
      {view.rows.length > 0 && <AddonTable rows={view.rows} unmanaged={view.unmanaged_folders} />}
      <Tools view={view} onDone={refresh} />
    </>
  );
}

function Summary({ view }: { view: AddonsView }) {
  const st = view.status!;
  const attention = st.missing.length + st.removed_elsewhere.length;
  return (
    <section>
      <div className="row">
        <h2 style={{ margin: 0 }}>Addons</h2>
        <span className={`pill ${attention ? "warn" : "ok"}`}>
          {attention ? `${attention} need${attention === 1 ? "s" : ""} attention` : "All in sync"}
        </span>
      </div>
      <div className="stats">
        <Stat n={st.in_sync} label="synced" />
        <Stat n={st.missing.length} label="to install" hot={st.missing.length > 0} />
        <Stat n={st.removed_elsewhere.length} label="removed elsewhere" hot={st.removed_elsewhere.length > 0} />
        <Stat n={view.unmanaged_folders.length} label="not in WowUp" />
      </div>
      <p className="muted small">
        Forever Sync shares the list of addons WowUp manages, not the addon files. WowUp installs and updates them;
        settings for every addon sync either way.
      </p>
    </section>
  );
}

function Stat({ n, label, hot }: { n: number; label: string; hot?: boolean }) {
  return (
    <div className={hot ? "stat hot" : "stat"}>
      <strong>{n}</strong>
      <span>{label}</span>
    </div>
  );
}

function ToInstall({ missing, importString }: { missing: PackageFile[]; importString: string }) {
  const { copied, copy } = useCopy();
  return (
    <section className="callout">
      <div className="row">
        <h2 style={{ margin: 0 }}>
          {plural(missing.length, "addon")} to install
        </h2>
        <button className="primary" onClick={() => copy("missing", importString)}>
          {copied === "missing" ? "Copied" : "Copy WowUp import string"}
        </button>
      </div>
      <ul className="names">
        {missing.map((p) => (
          <li key={`${p.provider}:${p.id}`}>
            {p.name} <span className="muted small">{p.version}</span>
          </li>
        ))}
      </ul>
      <ol className="small muted steps">
        <li>Open WowUp and pick WoW Forever.</li>
        <li>My Addons → ⋯ → Import/Export Addons → Import.</li>
        <li>Paste, then Install. This list updates after the next sync.</li>
      </ol>
    </section>
  );
}

function RemovedElsewhere({
  addons,
  gameRunning,
  onDone,
}: {
  addons: PackageFile[];
  gameRunning: boolean;
  onDone: (msg: string) => void;
}) {
  const [selected, setSelected] = useState<AddonKey[]>(() => addons.map(keyOf));
  const [plan, setPlan] = useState<RemovalPlan | null>(null);
  const [busy, setBusy] = useState<"remove" | "keep" | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => setSelected(addons.map(keyOf)), [addons]);

  const toggle = (k: AddonKey) =>
    setSelected((cur) => (cur.some((c) => same(c, k)) ? cur.filter((c) => !same(c, k)) : [...cur, k]));
  const run = async (what: "remove" | "keep", f: () => Promise<void>) => {
    setBusy(what);
    setError(null);
    try {
      await f();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  };
  const review = () => run("remove", async () => setPlan(await api.addonRemovalPlan(selected)));
  const remove = () =>
    run("remove", async () => {
      const r = await api.removeAddons(selected);
      setPlan(null);
      onDone(removedMessage(r));
    });
  const keep = () =>
    run("keep", async () => {
      const kept = selected.length;
      await api.keepAddons(selected);
      onDone(`Kept ${plural(kept, "addon")} on the list. Your other computers will be asked to install ${kept === 1 ? "it" : "them"} again.`);
    });

  return (
    <section className="attention">
      <h2>{plural(addons.length, "addon")} removed on another computer</h2>
      <p className="muted small">
        WowUp’s import adds addons but never removes them. Remove them here (folders go to the trash and they come off
        WowUp’s list), or keep them and put them back on the list for every computer.
      </p>
      <ul className="checks">
        {addons.map((p) => {
          const k = keyOf(p);
          return (
            <li key={`${p.provider}:${p.id}`}>
              <label>
                <input type="checkbox" checked={selected.some((c) => same(c, k))} onChange={() => toggle(k)} />
                {p.name} <span className="muted small">{p.version}</span>
              </label>
            </li>
          );
        })}
      </ul>
      {plan ? (
        <div className="confirm">
          <p className="small">These AddOns folders move to a trash folder next to AddOns (WoW ignores it):</p>
          <p className="small">
            <code>{plan.folders.join(", ") || "none"}</code>
          </p>
          {plan.kept_shared.length > 0 && (
            <p className="muted small">Kept because another addon still uses them: {plan.kept_shared.join(", ")}</p>
          )}
          <p className="muted small">
            {plural(plan.packages.length, "addon")} also come{plan.packages.length === 1 ? "s" : ""} off WowUp’s list, so
            WowUp won’t reinstall {plan.packages.length === 1 ? "it" : "them"}
            {plan.wowup_leftovers.length > 0 &&
              `, along with ${plural(plan.wowup_leftovers.length, "addon")} an earlier removal left there (${plan.wowup_leftovers
                .map((p) => p.name)
                .join(", ")})`}
            . {WOWUP_OPEN_HINT}
          </p>
          <div className="row">
            <button onClick={() => setPlan(null)} disabled={!!busy}>
              Cancel
            </button>
            <button className="primary" onClick={remove} disabled={!!busy || plan.packages.length === 0}>
              {busy === "remove"
                ? "Removing…"
                : plan.folders.length > 0
                  ? `Move ${plural(plan.folders.length, "folder")} to trash`
                  : "Remove from WowUp’s list"}
            </button>
          </div>
        </div>
      ) : (
        <div className="actions">
          <button className="primary" onClick={review} disabled={!!busy || !selected.length || gameRunning}>
            Remove {selected.length === addons.length ? "all" : selected.length} from this computer…
          </button>
          <button onClick={keep} disabled={!!busy || !selected.length}>
            {busy === "keep" ? "Keeping…" : "Keep on all computers"}
          </button>
        </div>
      )}
      {gameRunning && <p className="warn small">Quit WoW Forever to remove addons.</p>}
      {error && <p className="error">{error}</p>}
    </section>
  );
}

const WOWUP_OPEN_HINT = "If WowUp is open, restart it so its list catches up.";

function removedMessage(r: RemovalReport): string {
  const parts = [];
  if (r.moved.length > 0) parts.push(`Moved ${plural(r.moved.length, "folder")} to the addon trash (kept for 30 days).`);
  if (r.forgotten.length > 0)
    parts.push(`Removed ${plural(r.forgotten.length, "addon")} from WowUp’s list. ${WOWUP_OPEN_HINT}`);
  return parts.join(" ") || "Nothing needed removing.";
}

/** WowUp records an older version left behind when it moved addons to the trash. */
function Leftovers({ addons, onDone }: { addons: PackageFile[]; onDone: (msg: string) => void }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const clear = async () => {
    setBusy(true);
    setError(null);
    try {
      onDone(removedMessage(await api.removeAddons([])));
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <section className="attention">
      <h2>WowUp still lists {plural(addons.length, "removed addon")}</h2>
      <p className="muted small">
        Their folders are gone and they’re off the synced list, but WowUp still has them and would reinstall them on its
        next update: {addons.map((p) => p.name).join(", ")}.
      </p>
      <div className="actions">
        <button className="primary" onClick={clear} disabled={busy}>
          {busy ? "Clearing…" : "Clear from WowUp"}
        </button>
      </div>
      {error && <p className="error">{error}</p>}
    </section>
  );
}

function Pending({ status, onDone }: { status: NonNullable<AddonsView["status"]>; onDone: (msg?: string) => void }) {
  const [busy, setBusy] = useState(false);
  const sync = async () => {
    setBusy(true);
    try {
      await api.syncNow();
      onDone();
    } finally {
      setBusy(false);
    }
  };
  return (
    <section>
      <div className="row">
        <h2 style={{ margin: 0 }}>Waiting for the next sync</h2>
        <button className="small" onClick={sync} disabled={busy}>
          {busy ? "Syncing…" : "Sync now"}
        </button>
      </div>
      {status.new_here.length > 0 && (
        <p className="small">
          <strong>Added to the list:</strong> {status.new_here.map((p) => p.name).join(", ")}
        </p>
      )}
      {status.removed_here.length > 0 && (
        <p className="small">
          <strong>Removed from the list</strong> (uninstalled here): {status.removed_here.map((p) => p.name).join(", ")}
        </p>
      )}
    </section>
  );
}

type Filter = "all" | "attention" | "different";

function AddonTable({ rows, unmanaged }: { rows: AddonRow[]; unmanaged: string[] }) {
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<Filter>("all");
  const attention = (r: AddonRow) => r.state !== "in_sync";
  const shown = useMemo(() => {
    const q = query.trim().toLowerCase();
    return rows.filter(
      (r) =>
        (!q || r.name.toLowerCase().includes(q) || r.folders.some((f) => f.toLowerCase().includes(q))) &&
        (filter === "all" || (filter === "attention" ? attention(r) : r.other_version)),
    );
  }, [rows, query, filter]);
  const counts = { all: rows.length, attention: rows.filter(attention).length, different: rows.filter((r) => r.other_version).length };

  return (
    <section>
      <div className="row">
        <h2 style={{ margin: 0 }}>All addons</h2>
        <input className="search" placeholder="Search" value={query} onChange={(e) => setQuery(e.target.value)} />
      </div>
      <nav className="chips">
        {(
          [
            ["all", "All"],
            ["attention", "Needs attention"],
            ["different", "Other version elsewhere"],
          ] as [Filter, string][]
        ).map(([f, label]) => (
          <button key={f} className={filter === f ? "chip active" : "chip"} onClick={() => setFilter(f)} disabled={!counts[f] && f !== "all"}>
            {label} <span className="muted">{counts[f]}</span>
          </button>
        ))}
      </nav>
      <table className="addons">
        <thead>
          <tr>
            <th>Addon</th>
            <th>This computer</th>
            <th>Synced list</th>
            <th />
          </tr>
        </thead>
        <tbody>
          {shown.map((r) => (
            <tr key={`${r.provider}:${r.id}`}>
              <td>
                {r.name}
                <div className="muted tiny">{r.provider}</div>
              </td>
              <td className="ver">{r.version_here ?? <span className="muted">not installed</span>}</td>
              <td className={r.other_version ? "ver differs" : "ver"} title={r.other_version ? "Another computer has a different release. WowUp updates each computer on its own." : undefined}>
                {r.version_synced ?? <span className="muted">not on list</span>}
              </td>
              <td>
                <span className={`tag ${STATE[r.state].cls}`} title={STATE[r.state].hint}>
                  {STATE[r.state].label}
                </span>
              </td>
            </tr>
          ))}
          {!shown.length && (
            <tr>
              <td colSpan={4} className="muted small">
                No addons match.
              </td>
            </tr>
          )}
        </tbody>
      </table>
      {unmanaged.length > 0 && (
        <details className="small">
          <summary>{plural(unmanaged.length, "folder")} not managed by WowUp (not synced)</summary>
          <p className="muted">
            {unmanaged.join(", ")}
          </p>
          <p className="muted">Install these through WowUp if you want them on your other computers.</p>
        </details>
      )}
    </section>
  );
}

function Tools({ view, onDone }: { view: AddonsView; onDone: (msg?: string) => void }) {
  const { copied, copy } = useCopy();
  const installed = view.rows.filter((r) => r.version_here !== null).length;
  return (
    <section>
      <h2>Tools</h2>
      <ul className="tools">
        {view.import_string && (
          <li>
            <button className="link" onClick={() => copy("all", view.import_string!)}>
              {copied === "all" ? "Copied" : "Copy WowUp import string for every addon here"}
            </button>
            <span className="muted small"> Useful for a fresh WowUp install.</span>
          </li>
        )}
        {view.trash && (
          <li>
            <button className="link" onClick={() => api.openAddonTrash()}>
              Open the addon trash
            </button>
            <span className="muted small">
              {" "}
              {plural(view.trash.batches, "removal")} kept for 30 days. Move a folder back into AddOns to restore it.
            </span>
          </li>
        )}
        {view.status && (
          <li>
            <MakeAddonSource count={installed} onDone={onDone} />
          </li>
        )}
      </ul>
      {view.wowup_dir && <p className="muted tiny">WowUp data: {view.wowup_dir}</p>}
    </section>
  );
}

function MakeAddonSource({ count, onDone }: { count: number; onDone: (msg?: string) => void }) {
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);
  const go = async () => {
    setBusy(true);
    setMsg(null);
    try {
      await api.makeAddonSource();
      onDone("The synced list now matches this computer. Your other computers show what to install or remove after their next sync.");
    } catch (e) {
      setMsg(String(e));
    } finally {
      setBusy(false);
      setConfirming(false);
    }
  };
  if (!confirming)
    return (
      <>
        <button className="link" onClick={() => setConfirming(true)}>
          Make this computer’s addons the list to follow…
        </button>
        {msg && <span className="muted small"> {msg}</span>}
      </>
    );
  return (
    <div className="confirm">
      <p className="small">
        The synced list becomes exactly the {plural(count, "addon")} installed here. Addons that came only from your
        other computers are dropped from the list, and those computers will be asked to remove them. No addon files are
        deleted anywhere.
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
