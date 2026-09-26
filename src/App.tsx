import { AppShell } from "./components/layout/AppShell";
import { Sidebar } from "./components/sidebar/Sidebar";
import { DocumentGrid } from "./components/grid/DocumentGrid";
import { ScriptConsole } from "./components/console/ScriptConsole";
import { IndexesPanel } from "./components/indexes/IndexesPanel";
import { CollectionTabs } from "./components/tabs/CollectionTabs";
import { useConnectionsStore } from "./store/connectionsStore";
import { selectActiveTab, useSessionsStore } from "./store/sessionsStore";
import { useUiStore } from "./store/uiStore";
import type { MainTab } from "./store/uiStore";
import { useSuppressNativeContextMenu } from "./lib/useSuppressNativeContextMenu";

const tabLabels: Record<MainTab, string> = {
  browse: "Browse",
  indexes: "Indexes",
  console: "Console",
};

function App() {
  useSuppressNativeContextMenu();
  const sessions = useConnectionsStore((s) => s.sessions);
  const { mainTab, setMainTab } = useUiStore();
  const tabs = useSessionsStore((s) => s.tabs);
  const activeTab = useSessionsStore(selectActiveTab);

  const connected = Object.keys(sessions).length;
  const view: MainTab = connected === 0 ? "browse" : mainTab;
  // The status bar speaks for the active tab's server when there is one.
  const activeSession = activeTab ? sessions[activeTab.connection.id] : undefined;

  return (
    <AppShell
      sidebar={<Sidebar />}
      statusBar={
        <span>
          {activeTab && activeSession
            ? `Connected to ${activeTab.connection.name}${
                activeSession.serverVersion ? ` · MongoDB ${activeSession.serverVersion}` : ""
              }${connected > 1 ? ` · ${connected} connections open` : ""}`
            : connected > 0
              ? `${connected} connection${connected === 1 ? "" : "s"} open`
              : "Not connected"}
        </span>
      }
    >
      <div className="flex h-full flex-col">
        {connected > 0 && <CollectionTabs />}
        {/* Browse, Indexes and Console belong to a collection tab; a console
            tab is only a console. */}
        {activeTab?.kind === "collection" && (
          <div className="flex gap-1 border-b border-border-subtle bg-editor px-2 pt-1.5">
            {(["browse", "indexes", "console"] as const).map((t) => (
              <button
                key={t}
                type="button"
                className={`rounded-t px-3 py-1 text-xs ${
                  mainTab === t
                    ? "bg-panel text-text-default"
                    : "text-text-muted hover:text-text-default"
                }`}
                onClick={() => setMainTab(t)}
              >
                {tabLabels[t]}
              </button>
            ))}
          </div>
        )}
        <div className="relative min-h-0 flex-1">
          {/* Every tab's grid stays mounted and only the active one shows, so
              switching back keeps its scroll position and expanded nodes. */}
          {tabs.map(
            (tab) =>
              tab.kind === "collection" && (
                <div
                  key={tab.id}
                  className={`absolute inset-0 ${
                    view === "browse" && tab.id === activeTab?.id ? "" : "invisible"
                  }`}
                >
                  <DocumentGrid tab={tab} />
                </div>
              ),
          )}
          {activeTab?.kind === "collection" && view === "indexes" && (
            <IndexesPanel key={activeTab.id} tab={activeTab} />
          )}
          {activeTab && (activeTab.kind === "console" || view === "console") && (
            <ScriptConsole tab={activeTab} />
          )}
          {!activeTab && (
            <div className="flex h-full items-center justify-center px-6 text-center text-text-muted">
              {connected > 0
                ? "Open a collection, or right-click a database or collection to open a console on it"
                : "Connect to a server to browse its collections"}
            </div>
          )}
        </div>
      </div>
    </AppShell>
  );
}

export default App;
