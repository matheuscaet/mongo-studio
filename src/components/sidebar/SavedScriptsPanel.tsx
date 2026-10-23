import { useEffect } from "react";
import { RefreshCw } from "lucide-react";
import { useScriptsStore } from "../../store/scriptsStore";
import { useSessionsStore } from "../../store/sessionsStore";
import { Button } from "@/components/ui/button";
import { Kbd } from "@/components/ui/kbd";
import { PanelHeader } from "./PanelHeader";
import { PanelNotice } from "./PanelNotice";
import { SavedScriptRow } from "./SavedScriptRow";

/** The Saved scripts tool window: console scripts saved to disk. */
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

  useEffect(() => {
    refresh();
  }, [refresh]);

  return (
    <div className="flex h-full min-h-0 flex-col bg-panel">
      <PanelHeader title="Saved scripts">
        <Button
          variant="ghost"
          size="icon"
          title="Refresh"
          aria-label="Refresh saved scripts"
          onClick={() => refresh()}
        >
          <RefreshCw className="size-3.5" />
        </Button>
      </PanelHeader>

      {listError && <PanelNotice tone="danger">{listError}</PanelNotice>}

      <div className="min-h-0 flex-1 overflow-y-auto pb-3">
        {saved.length === 0 ? (
          <p className="px-3 py-1.5 text-sm text-fg-3">No saved scripts yet.</p>
        ) : (
          <div role="list" aria-label="Saved scripts">
            {saved.map((script) => (
              <div role="listitem" key={script.path}>
                <SavedScriptRow
                  script={script}
                  current={script.path === currentPath}
                  onOpen={() => open(script)}
                />
              </div>
            ))}
          </div>
        )}
        <p className="px-3 py-2.5 text-sm leading-normal text-fg-3">
          Opening a script starts a console on the database in view; its target pill moves it to
          another database or connection. <Kbd>Ctrl S</Kbd> saves the
          console you are in; scripts saved without a location land in{" "}
          <span className="font-data">~/mongo-studio-scripts</span>.
        </p>
      </div>
    </div>
  );
}
