import { useEffect, useState } from "react";
import { ChevronRight, FileCode, RefreshCw } from "lucide-react";
import { useScriptsStore } from "../../store/scriptsStore";
import { useSessionsStore } from "../../store/sessionsStore";

/** Console scripts saved to disk, pinned to the bottom of the sidebar. */
export function SavedScriptsPanel() {
  const saved = useScriptsStore((s) => s.saved);
  // highlight the file open in the console on screen - the active tab's
  const consoleKey = useSessionsStore((s) => s.activeTabId);
  const currentPath = useScriptsStore((s) =>
    consoleKey === null ? null : (s.files[consoleKey]?.path ?? null),
  );
  const listError = useScriptsStore((s) => s.listError);
  const refresh = useScriptsStore((s) => s.refresh);
  const open = useScriptsStore((s) => s.open);
  const [expanded, setExpanded] = useState(true);

  useEffect(() => {
    refresh();
  }, [refresh]);

  return (
    // Capped so a long list can't push the connections off screen.
    <div className="flex max-h-[45%] shrink-0 flex-col border-t border-border-subtle">
      <div className="flex items-center justify-between px-3 py-1.5">
        <button
          type="button"
          aria-expanded={expanded}
          className="flex items-center gap-1 text-[11px] font-bold uppercase tracking-wide text-text-muted hover:text-text-default"
          onClick={() => setExpanded((e) => !e)}
        >
          <ChevronRight
            size={12}
            className={`transition-transform ${expanded ? "rotate-90" : ""}`}
          />
          Saved scripts ({saved.length})
        </button>
        <button
          type="button"
          className="rounded p-1 text-text-muted hover:bg-sidebar-hover hover:text-text-default"
          onClick={() => refresh()}
          title="Refresh"
          aria-label="Refresh saved scripts"
        >
          <RefreshCw size={12} />
        </button>
      </div>

      {expanded && (
        <div className="min-h-0 overflow-y-auto pb-2">
          {listError && (
            <p className="px-3 py-1 text-xs text-red-400" title={listError}>
              {listError}
            </p>
          )}
          {saved.length === 0 ? (
            <p className="px-3 py-1 text-xs text-text-faint">
              Press Ctrl+S (⌘S) in the console to save a script. Scripts saved without
              picking a location go to ~/mongo-studio-scripts.
            </p>
          ) : (
            saved.map((script) => (
              <button
                key={script.path}
                type="button"
                title={script.path}
                className={`flex w-full items-center gap-1.5 px-3 py-1 text-left text-xs ${
                  script.path === currentPath
                    ? "bg-text-default/15 text-text-default"
                    : "text-text-muted hover:bg-sidebar-hover hover:text-text-default"
                }`}
                onClick={() => open(script)}
              >
                <FileCode size={12} className="shrink-0 text-text-faint" />
                <span className="truncate">{script.name}</span>
              </button>
            ))
          )}
        </div>
      )}
    </div>
  );
}
