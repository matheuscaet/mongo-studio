import { useState } from "react";
import { useConnectionsStore } from "../../store/connectionsStore";
import type { CollectionTab } from "../../store/sessionsStore";
import { useExportStore } from "../../store/exportStore";
import type { ExportNestedMode } from "../../types/export";

interface ExportDialogProps {
  tab: CollectionTab;
  onClose: () => void;
}

function currentQuery(
  s: CollectionTab,
  capToLimit: boolean,
): {
  filter: unknown;
  sort: unknown | null;
  pipeline: unknown | null;
  limit: number | null;
} {
  if (s.mode === "aggregate") {
    const trimmed = s.pipelineText.trim();
    return { filter: {}, sort: null, pipeline: trimmed ? JSON.parse(trimmed) : [], limit: null };
  }
  const filter = s.filterText.trim() ? JSON.parse(s.filterText) : {};
  const sort = s.sortText.trim() ? JSON.parse(s.sortText) : null;
  return { filter, sort, pipeline: null, limit: capToLimit ? s.limit : null };
}

export function ExportDialog({ tab, onClose }: ExportDialogProps) {
  const session = useConnectionsStore((st) => st.sessions[tab.connection.id]);
  const { database: selectedDatabase, collection: selectedCollection, mode, limit } = tab;
  const { running, rowsWritten, summary, error, start, cancel, reset } = useExportStore();
  const [nestedMode, setNestedMode] = useState<ExportNestedMode>("flatten");
  const [capToLimit, setCapToLimit] = useState(false);

  if (!session || !selectedDatabase || !selectedCollection) return null;

  async function handleExport() {
    if (!session || !selectedDatabase || !selectedCollection) return;
    let query;
    try {
      query = currentQuery(tab, capToLimit);
    } catch {
      useExportStore.setState({ error: "Current filter/pipeline is not valid JSON" });
      return;
    }
    await start(
      session.sessionId,
      selectedDatabase,
      selectedCollection,
      query,
      nestedMode,
      `${selectedCollection}.csv`,
    );
  }

  function handleClose() {
    reset();
    onClose();
  }

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50">
      <div className="w-full max-w-md rounded-lg border border-border-subtle bg-panel p-4 shadow-xl">
        <h2 className="mb-1 text-sm font-semibold text-text-default">Export to CSV</h2>
        <p className="mb-4 text-xs text-text-muted">
          {selectedDatabase}.{selectedCollection} - exports the current filter/pipeline, not
          just the page shown.
        </p>

        <div className="mb-4 flex flex-col gap-2">
          <label className="flex items-center gap-2 text-xs text-text-default">
            <input
              type="radio"
              checked={nestedMode === "flatten"}
              onChange={() => setNestedMode("flatten")}
              disabled={running}
            />
            Flatten nested fields into columns (address.city, items.0.sku)
          </label>
          <label className="flex items-center gap-2 text-xs text-text-default">
            <input
              type="radio"
              checked={nestedMode === "stringify"}
              onChange={() => setNestedMode("stringify")}
              disabled={running}
            />
            Keep nested fields as JSON text in one column
          </label>
          {mode === "find" && (
            <label className="flex items-center gap-2 text-xs text-text-default">
              <input
                type="checkbox"
                checked={capToLimit}
                onChange={(e) => setCapToLimit(e.target.checked)}
                disabled={running}
              />
              Cap to current limit ({limit})
            </label>
          )}
        </div>

        {running && (
          <p className="mb-3 text-xs text-text-muted">
            Exporting... {rowsWritten.toLocaleString()} rows written
          </p>
        )}
        {summary && (
          <p className="mb-3 text-xs text-emerald-400">
            Done - {summary.rowsWritten.toLocaleString()} rows, {summary.columns.length} columns
          </p>
        )}
        {error && <p className="mb-3 text-xs text-red-400">{error}</p>}

        <div className="flex justify-end gap-2">
          {running ? (
            <button
              type="button"
              className="rounded bg-panel-alt px-3 py-1.5 text-xs text-text-default hover:bg-panel-hover"
              onClick={() => cancel()}
            >
              Cancel
            </button>
          ) : (
            <>
              <button
                type="button"
                className="rounded bg-panel-alt px-3 py-1.5 text-xs text-text-default hover:bg-panel-hover"
                onClick={handleClose}
              >
                Close
              </button>
              <button
                type="button"
                className="rounded bg-run px-3 py-1.5 text-xs text-white hover:bg-run-hover"
                onClick={handleExport}
              >
                {summary ? "Export again" : "Export"}
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
