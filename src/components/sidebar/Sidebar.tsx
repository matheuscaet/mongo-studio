import { useEffect, useState } from "react";
import { ArrowLeftRight, FolderPlus, ListChecks, Plus, Trash2, X } from "lucide-react";
import { deleteConnections, useConnectionSelection } from "../../store/connectionSelectionStore";
import { sessionIdFor, useConnectionsStore } from "../../store/connectionsStore";
import { useSidebarLayoutStore } from "../../store/sidebarLayoutStore";
import { useSessionsStore } from "../../store/sessionsStore";
import { useUiStore } from "../../store/uiStore";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { cn } from "@/lib/utils";
import { groupState } from "../connections/ConnectionTreePicker";
import { ConnectionTree } from "./ConnectionTree";
import { ExplorerSearch } from "./ExplorerSearch";
import { PanelHeader } from "./PanelHeader";
import { PanelNotice } from "./PanelNotice";
import { useCollectionMatches } from "./useCollectionMatches";
import { TREE_SCROLLER_ATTR } from "./useWindowedRows";

function newConnection() {
  useUiStore.getState().setConnectionDialog({ mode: "new" });
}

/** The Explorer tool window: saved connections, their databases and collections. */
export function Sidebar() {
  const { profiles, profilesLoaded, refreshProfiles, loadSecretBackendInfo, secretBackend } =
    useConnectionsStore();
  const layoutLoaded = useSidebarLayoutStore((s) => s.loaded);
  const layoutError = useSidebarLayoutStore((s) => s.error);
  const createFolder = useSidebarLayoutStore((s) => s.createFolder);
  const [search, setSearch] = useState("");
  const collectionMatches = useCollectionMatches(search.trim().toLowerCase());
  const selectedIds = useConnectionSelection((s) => s.ids);
  const selecting = useConnectionSelection((s) => s.selecting);
  const setSelecting = useConnectionSelection((s) => s.setSelecting);
  const allIds = profiles.map((p) => p.id);
  const allState = groupState(allIds, (id) => selectedIds.includes(id));
  // only the picks still saved: one deleted elsewhere drops out
  const picked = selectedIds.filter((id) => profiles.some((p) => p.id === id));

  // Enter opens the first collection found, so a few letters and Enter get
  // you to a collection without the mouse.
  function openFirstMatch() {
    if (!collectionMatches) return;
    const { connectionId, database, collection } = collectionMatches.first;
    const profile = profiles.find((p) => p.id === connectionId);
    const sessionId = sessionIdFor(connectionId);
    if (!profile || !sessionId) return;
    useSessionsStore
      .getState()
      .openCollection(
        sessionId,
        { id: profile.id, name: profile.name, summary: profile.summary },
        database,
        collection,
      );
  }

  useEffect(() => {
    refreshProfiles();
    loadSecretBackendInfo();
    useSidebarLayoutStore.getState().load();
  }, [refreshProfiles, loadSecretBackendInfo]);

  // Place new connections and drop deleted ones - but only once the list has
  // really loaded: an empty list before that would pull every connection out
  // of its folder.
  const profileIds = profiles.map((p) => p.id).join("\n");
  useEffect(() => {
    if (profilesLoaded && layoutLoaded) {
      useSidebarLayoutStore.getState().sync(profileIds ? profileIds.split("\n") : []);
    }
  }, [profilesLoaded, layoutLoaded, profileIds]);

  return (
    <div className="flex h-full min-h-0 flex-col bg-panel">
      <PanelHeader title="Explorer">
        <Button
          variant="ghost"
          size="icon"
          title={selecting ? "Stop selecting (Esc)" : "Select connections"}
          aria-label="Select connections"
          aria-pressed={selecting}
          disabled={profiles.length === 0}
          className={cn(selecting && "bg-sel text-fg")}
          onClick={() => setSelecting(!selecting)}
        >
          <ListChecks />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          title="New folder"
          aria-label="New folder"
          onClick={() => createFolder(null)}
        >
          <FolderPlus />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          title="Import / export connections"
          aria-label="Import or export connections"
          onClick={() => useUiStore.getState().openTransfer("export")}
        >
          <ArrowLeftRight />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          title="New connection (Ctrl N)"
          aria-label="New connection"
          onClick={newConnection}
        >
          <Plus />
        </Button>
      </PanelHeader>

      <ExplorerSearch value={search} onChange={setSearch} onSubmit={openFirstMatch} />

      {secretBackend?.warning && <PanelNotice>{secretBackend.warning}</PanelNotice>}
      {layoutError && <PanelNotice>{layoutError}</PanelNotice>}

      <div className="min-h-0 flex-1 overflow-y-auto" {...{ [TREE_SCROLLER_ATTR]: "" }}>
        {profiles.length === 0 && profilesLoaded ? (
          <div className="flex flex-col items-start gap-2 px-3 py-2.5 text-sm text-fg-3">
            <p>No connections yet. Add one to browse its databases.</p>
            <Button size="sm" onClick={newConnection}>
              <Plus />
              New connection
            </Button>
          </div>
        ) : (
          <ConnectionTree search={search} />
        )}
      </div>

      {(selecting || picked.length > 0) && (
        <div
          role="toolbar"
          aria-label="Selected connections"
          className="flex h-9 shrink-0 items-center gap-1.5 border-t border-line px-2 text-sm text-fg-2"
        >
          {selecting && (
            <Checkbox
              checked={allState}
              aria-label="Select all connections"
              title="Select all"
              onCheckedChange={() => useConnectionSelection.getState().setMany(allIds, allState !== true)}
            />
          )}
          <span className="min-w-0 flex-1 truncate tabular-nums">
            {picked.length === 1 ? "1 connection" : `${picked.length} connections`} selected
          </span>
          <Button
            size="sm"
            variant="destructive"
            disabled={picked.length === 0}
            onClick={() => void deleteConnections(picked)}
          >
            <Trash2 />
            Delete
          </Button>
          <Button
            variant="ghost"
            size="icon-sm"
            title={selecting ? "Done (Esc)" : "Clear selection (Esc)"}
            aria-label={selecting ? "Done selecting" : "Clear selection"}
            onClick={() => setSelecting(false)}
          >
            <X />
          </Button>
        </div>
      )}
    </div>
  );
}
