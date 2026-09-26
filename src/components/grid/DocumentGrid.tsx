import { memo, useMemo, useState } from "react";
import { useConnectionsStore } from "../../store/connectionsStore";
import { useSessionsStore } from "../../store/sessionsStore";
import type { CollectionTab } from "../../store/sessionsStore";
import { api } from "../../lib/tauri";
import { ValueEditContext } from "../json/EditableValue";
import type { ValueEditor } from "../json/EditableValue";
import { QueryBar } from "../query/QueryBar";
import { ExportDialog } from "../export/ExportDialog";
import { DocumentCard } from "./DocumentCard";
import { DocumentActions } from "./DocumentActions";
import { JsonTable } from "../json/JsonTable";
import { ResultViewToggle } from "../json/ResultViewToggle";
import { useUiStore } from "../../store/uiStore";

// Memoised because every open tab has a grid mounted: typing in one tab's
// query must not re-render the others, whose tab objects are unchanged.
export const DocumentGrid = memo(function DocumentGrid({ tab }: { tab: CollectionTab }) {
  const {
    database: selectedDatabase,
    collection: selectedCollection,
    stats,
    results,
    error,
    loading,
  } = tab;
  const [exportOpen, setExportOpen] = useState(false);
  const resultView = useUiStore((s) => s.resultView);
  const session = useConnectionsStore((s) => s.sessions[tab.connection.id]);
  const replaceDocument = useSessionsStore((s) => s.replaceDocument);

  // Values are editable only in find results; see CollectionTab.resultsMode.
  const valueEditor = useMemo<ValueEditor | null>(() => {
    if (!session || tab.resultsMode !== "find") return null;
    return {
      commit: async (doc, path, value) => {
        const id = (doc as { _id?: unknown } | null)?._id;
        if (id === undefined) throw new Error("This document has no _id to update it by");
        const updated = await api.updateField(
          session.sessionId,
          tab.database,
          tab.collection,
          id,
          path,
          value,
        );
        replaceDocument(tab.id, updated);
      },
    };
  }, [session, tab.id, tab.database, tab.collection, tab.resultsMode, replaceDocument]);

  return (
    <div className="flex h-full flex-col">
      <QueryBar tab={tab} />
      <div className="flex items-center justify-between border-b border-border-subtle bg-editor px-3 py-1.5 text-xs text-text-muted">
        <div className="flex items-center gap-3">
          <span className="font-mono text-text-default">
            {selectedDatabase}.{selectedCollection}
          </span>
          {stats && (
            <>
              <span>{stats.documentCount.toLocaleString()} documents</span>
              <span>{stats.indexes.length} indexes</span>
            </>
          )}
          {loading && <span>Loading…</span>}
          {results && <span>Showing {results.returned}</span>}
        </div>
        <div className="flex items-center gap-2">
          <ResultViewToggle />
          <button
            type="button"
            className="rounded border border-border-subtle px-2 py-1 text-text-default hover:bg-panel-hover"
            onClick={() => setExportOpen(true)}
          >
            Export CSV
          </button>
        </div>
      </div>
      {error && (
        <div className="m-2 rounded bg-red-950 p-2 text-xs text-red-300">{error}</div>
      )}
      <ValueEditContext.Provider value={valueEditor}>
        <div className="flex-1 overflow-y-auto">
          {results?.documents.length === 0 && (
            <p className="p-3 text-xs text-text-faint">No documents match this query.</p>
          )}
          {resultView === "table" ? (
            results &&
            results.documents.length > 0 && (
              <JsonTable
                value={results.documents}
                rootActions={(doc) => (
                  <DocumentActions
                    doc={doc}
                    collectionName={selectedCollection}
                    tabId={tab.id}
                  />
                )}
              />
            )
          ) : (
            <div className="flex flex-col gap-2 p-2">
              {results?.documents.map((doc, i) => (
                <DocumentCard
                  key={i}
                  doc={doc}
                  collectionName={selectedCollection}
                  tabId={tab.id}
                />
              ))}
            </div>
          )}
        </div>
      </ValueEditContext.Provider>
      {exportOpen && <ExportDialog tab={tab} onClose={() => setExportOpen(false)} />}
    </div>
  );
});
