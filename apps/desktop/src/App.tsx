import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./App.css";

type Character = { folder: string };
type Realm = { id: string; characters: Character[] };
type Account = { name: string; realms: Realm[] };
type ForeverInstall = { root: string; flavor_dir: string; version: string | null; accounts: Account[] };
type ManagedAddon = {
  name: string;
  provider: string;
  external_id: string;
  version_id: string | null;
  version: string | null;
  folders: string[];
};
type Inventory = { wowup_dir: string; managed: ManagedAddon[]; unmanaged_folders: string[] };
type Overview = {
  install: ForeverInstall;
  addons: Inventory | null;
  addons_error: string | null;
  import_string: string | null;
};

export default function App() {
  const [data, setData] = useState<Overview | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  const load = () => {
    setError(null);
    invoke<Overview>("overview").then(setData).catch((e) => setError(String(e)));
  };
  useEffect(load, []);

  const copy = async () => {
    if (!data?.import_string) return;
    await navigator.clipboard.writeText(data.import_string);
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };

  return (
    <main>
      <header>
        <h1>Forever Sync</h1>
        <button onClick={load}>Refresh</button>
      </header>

      {error && <p className="error">{error}</p>}
      {!data && !error && <p className="muted">Looking for WoW Forever…</p>}

      {data && (
        <>
          <section>
            <h2>Install</h2>
            <p>
              {data.install.root} <span className="muted">· client {data.install.version ?? "unknown"}</span>
            </p>
            {data.install.accounts.map((a) => (
              <div key={a.name}>
                <strong>{a.name}</strong>
                {a.realms.map((r) => (
                  <p key={r.id} className="muted">
                    Realm {r.id}: {r.characters.map((c) => c.folder).join(", ")}
                  </p>
                ))}
              </div>
            ))}
          </section>

          <section>
            <div className="row">
              <h2>Addons (via WowUp)</h2>
              <button onClick={copy} disabled={!data.import_string}>
                {copied ? "Copied" : "Copy WowUp import"}
              </button>
            </div>
            {data.addons_error && <p className="error">{data.addons_error}</p>}
            {data.addons && (
              <>
                <table>
                  <tbody>
                    {data.addons.managed.map((a) => (
                      <tr key={`${a.provider}:${a.external_id}`}>
                        <td>{a.name}</td>
                        <td className="muted">{a.provider}</td>
                        <td className="muted">{a.version ?? "–"}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
                {data.addons.unmanaged_folders.length > 0 && (
                  <p className="muted">
                    Not managed by WowUp (not synced): {data.addons.unmanaged_folders.join(", ")}
                  </p>
                )}
              </>
            )}
          </section>
        </>
      )}
    </main>
  );
}
