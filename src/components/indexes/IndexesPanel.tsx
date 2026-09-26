import { useEffect, useState } from "react";
import { useConnectionsStore } from "../../store/connectionsStore";
import type { CollectionTab } from "../../store/sessionsStore";
import { api } from "../../lib/tauri";

interface IndexStatEntry {
  name?: string;
  accesses?: { ops?: number; since?: string };
}

function isIndexStatEntry(value: unknown): value is IndexStatEntry {
  return typeof value === "object" && value !== null;
}

export function IndexesPanel({ tab }: { tab: CollectionTab }) {
  const session = useConnectionsStore((s) => s.sessions[tab.connection.id]);
  const { database: selectedDatabase, collection: selectedCollection, stats } = tab;
  const [statsByName, setStatsByName] = useState<Map<string, IndexStatEntry>>(new Map());
  const [statsError, setStatsError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    if (!session || !selectedDatabase || !selectedCollection) return;
    let cancelled = false;
    setLoading(true);
    setStatsError(null);
    api
      .listIndexStats(session.sessionId, selectedDatabase, selectedCollection)
      .then((entries) => {
        if (cancelled) return;
        const map = new Map<string, IndexStatEntry>();
        for (const entry of entries) {
          if (isIndexStatEntry(entry) && typeof entry.name === "string") {
            map.set(entry.name, entry);
          }
        }
        setStatsByName(map);
      })
      .catch((e) => {
        if (!cancelled) setStatsError(String(e));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [session, selectedDatabase, selectedCollection]);

  return (
    <div className="flex h-full flex-col overflow-y-auto p-3">
      <div className="mb-2 flex items-center justify-between">
        <span className="font-mono text-xs text-text-default">
          {selectedDatabase}.{selectedCollection}
        </span>
        {loading && <span className="text-xs text-text-faint">Loading usage stats…</span>}
      </div>
      {statsError && (
        <div className="mb-2 rounded bg-amber-950 p-2 text-xs text-amber-300">
          Index usage stats unavailable: {statsError}
        </div>
      )}
      <div className="overflow-x-auto rounded border border-border-subtle">
        <table className="w-full text-left text-xs">
          <thead className="bg-panel-alt text-text-muted">
            <tr>
              <th className="px-2 py-1.5 font-medium">Name</th>
              <th className="px-2 py-1.5 font-medium">Key</th>
              <th className="px-2 py-1.5 font-medium">Unique</th>
              <th className="px-2 py-1.5 font-medium">Ops since restart</th>
            </tr>
          </thead>
          <tbody>
            {stats?.indexes.map((index) => {
              const usage = statsByName.get(index.name);
              return (
                <tr key={index.name} className="border-t border-border-subtle">
                  <td className="px-2 py-1.5 text-text-default">{index.name}</td>
                  <td className="px-2 py-1.5 font-mono text-text-muted">
                    {JSON.stringify(index.key)}
                  </td>
                  <td className="px-2 py-1.5 text-text-muted">{index.unique ? "yes" : ""}</td>
                  <td className="px-2 py-1.5 text-text-muted">
                    {usage?.accesses?.ops?.toLocaleString() ?? "—"}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
      {(!stats || stats.indexes.length === 0) && (
        <p className="mt-2 text-xs text-text-faint">No indexes.</p>
      )}
    </div>
  );
}
