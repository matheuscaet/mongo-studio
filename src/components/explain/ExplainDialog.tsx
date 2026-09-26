import { useState } from "react";
import { useConnectionsStore } from "../../store/connectionsStore";
import type { CollectionTab } from "../../store/sessionsStore";
import { api } from "../../lib/tauri";
import { summarizeExplain } from "../../lib/explain";
import type { ExplainVerbosity } from "../../types/explain";

interface ExplainDialogProps {
  tab: CollectionTab;
  onClose: () => void;
}

const verbosityOptions: { id: ExplainVerbosity; label: string }[] = [
  { id: "query_planner", label: "Query planner" },
  { id: "execution_stats", label: "Execution stats" },
  { id: "all_plans_execution", label: "All plans execution" },
];

export function ExplainDialog({ tab, onClose }: ExplainDialogProps) {
  const session = useConnectionsStore((s) => s.sessions[tab.connection.id]);
  const {
    database: selectedDatabase,
    collection: selectedCollection,
    mode,
    filterText,
    sortText,
    pipelineText,
  } = tab;
  const [verbosity, setVerbosity] = useState<ExplainVerbosity>("execution_stats");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [raw, setRaw] = useState<unknown>(null);
  const [showRaw, setShowRaw] = useState(false);

  if (!session || !selectedDatabase || !selectedCollection) return null;

  async function handleRun() {
    setLoading(true);
    setError(null);
    setRaw(null);
    try {
      const query =
        mode === "aggregate"
          ? {
              filter: {},
              sort: null,
              projection: null,
              limit: null,
              skip: null,
              pipeline: pipelineText.trim() ? JSON.parse(pipelineText) : [],
            }
          : {
              filter: filterText.trim() ? JSON.parse(filterText) : {},
              sort: sortText.trim() ? JSON.parse(sortText) : null,
              projection: null,
              limit: null,
              skip: null,
              pipeline: null,
            };
      const result = await api.explainQuery(
        session!.sessionId,
        selectedDatabase!,
        selectedCollection!,
        query,
        verbosity,
      );
      setRaw(result);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }

  const summary = raw ? summarizeExplain(raw) : null;

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50">
      <div className="flex max-h-[80vh] w-full max-w-2xl flex-col rounded-lg border border-border-subtle bg-panel p-4 shadow-xl">
        <h2 className="mb-1 text-sm font-semibold text-text-default">Explain query</h2>
        <p className="mb-3 text-xs text-text-muted">
          {selectedDatabase}.{selectedCollection} - explains the current{" "}
          {mode === "aggregate" ? "pipeline" : "filter/sort"}.
        </p>

        <div className="mb-3 flex items-center gap-2">
          <select
            className="rounded border border-border-subtle bg-editor px-2 py-1 text-xs text-text-default"
            value={verbosity}
            onChange={(e) => setVerbosity(e.target.value as ExplainVerbosity)}
            disabled={loading}
          >
            {verbosityOptions.map((v) => (
              <option key={v.id} value={v.id}>
                {v.label}
              </option>
            ))}
          </select>
          <button
            type="button"
            disabled={loading}
            className="rounded bg-run px-3 py-1.5 text-xs text-white hover:bg-run-hover disabled:opacity-50"
            onClick={handleRun}
          >
            {loading ? "Running…" : "Run Explain"}
          </button>
        </div>

        {error && <p className="mb-2 text-xs text-red-400">{error}</p>}

        {summary && (
          <div className="mb-3 flex flex-col gap-1 rounded border border-border-subtle bg-editor p-2 text-xs">
            <div className="flex items-center gap-2">
              <span className="text-text-muted">Plan:</span>
              <span className="font-mono text-text-default">
                {summary.stages.length > 0 ? summary.stages.join(" → ") : "unknown"}
              </span>
            </div>
            <div className="flex items-center gap-2">
              <span className="text-text-muted">Index used:</span>
              {summary.usesCollectionScan ? (
                <span className="text-amber-400">
                  none - full collection scan (COLLSCAN)
                </span>
              ) : (
                <span className="font-mono text-emerald-400">
                  {summary.indexName ?? "unknown"}
                </span>
              )}
            </div>
            {summary.executionTimeMillis !== null && (
              <div className="flex flex-wrap gap-x-4 gap-y-1 text-text-muted">
                <span>returned: {summary.nReturned}</span>
                <span>keys examined: {summary.totalKeysExamined}</span>
                <span>docs examined: {summary.totalDocsExamined}</span>
                <span>time: {summary.executionTimeMillis}ms</span>
              </div>
            )}
          </div>
        )}

        {raw !== null && (
          <button
            type="button"
            className="mb-2 self-start text-xs text-text-muted underline hover:text-text-default"
            onClick={() => setShowRaw((v) => !v)}
          >
            {showRaw ? "Hide raw explain output" : "Show raw explain output"}
          </button>
        )}
        {showRaw && raw !== null && (
          <pre className="mb-3 flex-1 overflow-auto rounded border border-border-subtle bg-editor p-2 text-[11px] text-text-muted">
            {JSON.stringify(raw, null, 2)}
          </pre>
        )}

        <div className="mt-auto flex justify-end">
          <button
            type="button"
            className="rounded bg-panel-alt px-3 py-1.5 text-xs text-text-default hover:bg-panel-hover"
            onClick={onClose}
          >
            Close
          </button>
        </div>
      </div>
    </div>
  );
}
