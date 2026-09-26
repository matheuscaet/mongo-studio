import { useState } from "react";
import { useConnectionsStore } from "../../store/connectionsStore";
import { useSessionsStore } from "../../store/sessionsStore";
import type { CollectionTab, QueryMode } from "../../store/sessionsStore";
import { ExplainDialog } from "../explain/ExplainDialog";
import { QueryEditor } from "./QueryEditor";

const inputClass =
  "rounded border border-border-subtle bg-panel px-2 py-1 text-xs text-text-default focus:border-accent focus:outline-none";

const modes: { id: QueryMode; label: string }[] = [
  { id: "find", label: "Find" },
  { id: "aggregate", label: "Aggregate" },
];

export function QueryBar({ tab }: { tab: CollectionTab }) {
  const session = useConnectionsStore((s) => s.sessions[tab.connection.id]);
  const updateTab = useSessionsStore((s) => s.updateTab);
  const runQuery = useSessionsStore((s) => s.runQuery);
  const [explainOpen, setExplainOpen] = useState(false);

  if (!session) return null;

  const { mode, filterText, sortText, limit, skip, pipelineText, loading } = tab;
  const submit = () => runQuery(session.sessionId, tab.id);
  // Completion names come from this tab's collection.
  const completionContext = () => ({
    sessionId: session.sessionId,
    database: tab.database,
    collection: tab.collection,
  });

  return (
    <div className="border-b border-border-subtle bg-editor">
      <div className="flex gap-1 px-2 pt-1.5">
        {modes.map((m) => (
          <button
            key={m.id}
            type="button"
            className={`rounded-t px-2.5 py-1 text-[11px] ${
              mode === m.id
                ? "bg-panel text-text-default"
                : "text-text-muted hover:text-text-default"
            }`}
            onClick={() => updateTab(tab.id, { mode: m.id })}
          >
            {m.label}
          </button>
        ))}
      </div>
      <div className="flex flex-wrap items-end gap-2 bg-panel p-2">
        {mode === "find" ? (
          <>
            <div className="min-w-[200px] flex-1">
              <label className="mb-1 block text-[10px] uppercase tracking-wide text-text-muted">
                Filter (JSON)
              </label>
              <QueryEditor
                kind="filter"
                ariaLabel="Filter"
                value={filterText}
                onChange={(text) => updateTab(tab.id, { filterText: text })}
                completionContext={completionContext}
                onSubmit={submit}
                placeholder='{ "field": "value" }'
              />
            </div>
            <div className="min-w-[140px]">
              <label className="mb-1 block text-[10px] uppercase tracking-wide text-text-muted">
                Sort (JSON)
              </label>
              <QueryEditor
                kind="sort"
                ariaLabel="Sort"
                value={sortText}
                onChange={(text) => updateTab(tab.id, { sortText: text })}
                completionContext={completionContext}
                onSubmit={submit}
                placeholder='{ "_id": -1 }'
              />
            </div>
            <div>
              <label className="mb-1 block text-[10px] uppercase tracking-wide text-text-muted">
                Limit
              </label>
              <input
                type="number"
                className={`${inputClass} w-20`}
                value={limit}
                onChange={(e) => updateTab(tab.id, { limit: Number(e.target.value) })}
              />
            </div>
            <div>
              <label className="mb-1 block text-[10px] uppercase tracking-wide text-text-muted">
                Skip
              </label>
              <input
                type="number"
                className={`${inputClass} w-20`}
                value={skip}
                onChange={(e) => updateTab(tab.id, { skip: Number(e.target.value) })}
              />
            </div>
          </>
        ) : (
          <div className="min-w-[200px] flex-1">
            <label className="mb-1 block text-[10px] uppercase tracking-wide text-text-muted">
              Pipeline (JSON array of stages) - Ctrl+Enter runs
            </label>
            <QueryEditor
              kind="pipeline"
              ariaLabel="Pipeline"
              multiline
              value={pipelineText}
              onChange={(text) => updateTab(tab.id, { pipelineText: text })}
              completionContext={completionContext}
              onSubmit={submit}
              placeholder='[ { "$match": {} }, { "$limit": 50 } ]'
            />
          </div>
        )}
        <button
          type="button"
          disabled={loading}
          className="rounded bg-run px-3 py-1.5 text-xs text-white hover:bg-run-hover disabled:opacity-50"
          onClick={submit}
        >
          Run
        </button>
        <button
          type="button"
          className="rounded border border-border-subtle px-3 py-1.5 text-xs text-text-default hover:bg-panel-hover"
          onClick={() => setExplainOpen(true)}
        >
          Explain
        </button>
      </div>
      {explainOpen && <ExplainDialog tab={tab} onClose={() => setExplainOpen(false)} />}
    </div>
  );
}
