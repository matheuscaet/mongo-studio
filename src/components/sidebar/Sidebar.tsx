import { useEffect, useState } from "react";
import { ArrowLeftRight, FolderPlus, Plus, Search } from "lucide-react";
import { sessionIdFor, useConnectionsStore } from "../../store/connectionsStore";
import { useSidebarLayoutStore } from "../../store/sidebarLayoutStore";
import { useSessionsStore } from "../../store/sessionsStore";
import { api } from "../../lib/tauri";
import type { ConnectionProfile } from "../../types/connection";
import { ConnectionForm } from "../connections/ConnectionForm";
import { ConnectionsImportExportDialog } from "../connections/ConnectionsImportExportDialog";
import { ConnectionTree } from "./ConnectionTree";
import { SavedScriptsPanel } from "./SavedScriptsPanel";
import { ThemeSwitcher } from "./ThemeSwitcher";
import { useCollectionMatches } from "./useCollectionMatches";

export function Sidebar() {
  const { profiles, profilesLoaded, refreshProfiles, loadSecretBackendInfo, secretBackend } =
    useConnectionsStore();
  const layoutLoaded = useSidebarLayoutStore((s) => s.loaded);
  const layoutError = useSidebarLayoutStore((s) => s.error);
  const createFolder = useSidebarLayoutStore((s) => s.createFolder);
  const [showForm, setShowForm] = useState(false);
  const [showImportExport, setShowImportExport] = useState(false);
  const [editing, setEditing] = useState<ConnectionProfile | null>(null);
  const [editError, setEditError] = useState<string | null>(null);

  async function handleEdit(id: string) {
    setEditError(null);
    try {
      setEditing(await api.getConnectionProfile(id));
    } catch (e) {
      setEditError(String(e));
    }
  }
  const [search, setSearch] = useState("");
  const collectionMatches = useCollectionMatches(search.trim().toLowerCase());

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
    <div className="flex h-full flex-col bg-sidebar">
      {secretBackend?.warning && (
        <div className="border-b border-border-subtle bg-amber-950 p-2 text-xs text-amber-300">
          {secretBackend.warning}
        </div>
      )}

      <div className="flex items-center justify-between px-3 py-3">
        <span className="text-base font-bold text-text-default">Mongo Studio</span>
        <ThemeSwitcher />
      </div>

      <div className="flex items-center justify-between px-3 pb-1.5 pt-1">
        <span className="text-[11px] font-bold uppercase tracking-wide text-text-muted">
          Connections ({profiles.length})
        </span>
        <div className="flex items-center gap-0.5">
          <button
            type="button"
            className="rounded p-1 text-text-muted hover:bg-sidebar-hover hover:text-text-default"
            onClick={() => setShowImportExport(true)}
            title="Import / export connections"
          >
            <ArrowLeftRight size={14} />
          </button>
          <button
            type="button"
            className="rounded p-1 text-text-muted hover:bg-sidebar-hover hover:text-text-default"
            onClick={() => createFolder(null)}
            title="New folder"
          >
            <FolderPlus size={14} />
          </button>
          <button
            type="button"
            className="rounded p-1 text-text-muted hover:bg-sidebar-hover hover:text-text-default"
            onClick={() => setShowForm(true)}
            title="New connection"
          >
            <Plus size={15} />
          </button>
        </div>
      </div>

      <div className="px-3 pb-2">
        <div className="flex items-center gap-1.5 rounded border border-border-subtle bg-panel px-2 py-1">
          <Search size={12} className="shrink-0 text-text-faint" />
          <input
            className="w-full bg-transparent text-xs text-text-default placeholder:text-text-faint focus:outline-none"
            placeholder="Search connections and collections"
            aria-label="Search connections and collections"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") openFirstMatch();
              if (e.key === "Escape") setSearch("");
            }}
          />
        </div>
      </div>

      {editError && (
        <p className="mx-3 mb-2 rounded bg-red-950 p-2 text-xs text-red-300">{editError}</p>
      )}

      {layoutError && (
        <p className="mx-3 mb-2 rounded bg-amber-950 p-2 text-xs text-amber-300">{layoutError}</p>
      )}

      <div className="flex-1 overflow-y-auto">
        {profiles.length === 0 && profilesLoaded ? (
          <p className="px-3 py-2 text-xs text-text-faint">
            No connections yet. Click "+" to add one.
          </p>
        ) : (
          <ConnectionTree search={search} onEdit={handleEdit} />
        )}
      </div>

      <SavedScriptsPanel />

      {showForm && (
        <ConnectionForm
          onSaved={() => setShowForm(false)}
          onCancel={() => setShowForm(false)}
        />
      )}

      {editing && (
        <ConnectionForm
          editing={editing}
          onSaved={() => setEditing(null)}
          onCancel={() => setEditing(null)}
        />
      )}

      {showImportExport && (
        <ConnectionsImportExportDialog onClose={() => setShowImportExport(false)} />
      )}
    </div>
  );
}
