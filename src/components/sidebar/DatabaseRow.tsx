import { useCallback, useState } from "react";
import type { MouseEvent } from "react";
import { ChevronRight, Layers } from "lucide-react";
import { databaseKey, selectActiveTab, useSessionsStore } from "../../store/sessionsStore";
import type { TabConnection } from "../../store/sessionsStore";
import type { CollectionInfo, DatabaseInfo } from "../../types/connection";
import { ContextMenu } from "../ui/ContextMenu";

// The active connection already paints its whole subtree bg-sidebar-active,
// so the open collection can't reuse that colour. A wash of the text colour
// stands out on any theme: it lightens dark ones and darkens the light one.
const activeRowClass = "rounded bg-text-default/15";

/** The name with the part matching the search picked out. */
function Highlighted({ name, query }: { name: string; query: string }) {
  const at = query ? name.toLowerCase().indexOf(query) : -1;
  if (at < 0) return <>{name}</>;
  return (
    <>
      {name.slice(0, at)}
      <mark className="bg-transparent font-semibold text-text-default">
        {name.slice(at, at + query.length)}
      </mark>
      {name.slice(at + query.length)}
    </>
  );
}

interface DatabaseRowProps {
  db: DatabaseInfo;
  sessionId: string;
  connection: TabConnection;
  /** The sidebar search, lowercased. */
  query: string;
  /** This database's collections matching the search, when some do; it opens to show them. */
  matches: CollectionInfo[] | null;
}

export function DatabaseRow({ db, sessionId, connection, query, matches }: DatabaseRowProps) {
  const tree = useSessionsStore((s) => s.databaseTree[databaseKey(connection.id, db.name)]);
  // Strings, not the tab object, so typing in a query doesn't re-render the tree.
  const activeConnectionId = useSessionsStore(
    (s) => selectActiveTab(s)?.connection.id ?? null,
  );
  const activeDatabase = useSessionsStore((s) => selectActiveTab(s)?.database ?? null);
  // a console tab's collection is only where it started, not what it shows
  const activeTabCollection = useSessionsStore((s) => {
    const tab = selectActiveTab(s);
    return tab?.kind === "collection" ? tab.collection : null;
  });
  const toggleDatabase = useSessionsStore((s) => s.toggleDatabase);
  const openCollection = useSessionsStore((s) => s.openCollection);
  const openConsole = useSessionsStore((s) => s.openConsole);
  // Right-click menu, on the database row or one of its collections.
  const [menu, setMenu] = useState<{ x: number; y: number; collection: string | null } | null>(
    null,
  );
  const closeMenu = useCallback(() => setMenu(null), []);

  function openMenu(e: MouseEvent, collection: string | null) {
    e.preventDefault();
    e.stopPropagation();
    setMenu({ x: e.clientX, y: e.clientY, collection });
  }

  const isOpen = (tree?.expanded ?? false) || matches !== null;
  const collections = tree?.collections ?? [];
  const collectionsLoading = tree?.loading ?? false;
  const collectionsError = tree?.error ?? null;
  // Collapsing is only visual, so surface the active tab's collection on the
  // database row itself - otherwise it disappears from the sidebar.
  const activeCollection =
    activeConnectionId === connection.id && activeDatabase === db.name
      ? activeTabCollection
      : null;
  const showsActiveInline = !isOpen && activeCollection !== null;

  return (
    <div>
      <button
        type="button"
        className={`flex w-full items-center gap-1.5 px-1.5 py-1 text-left text-xs text-text-default ${
          showsActiveInline ? activeRowClass : "hover:bg-sidebar-hover"
        }`}
        onClick={() => toggleDatabase(connection.id, sessionId, db.name)}
        onContextMenu={(e) => openMenu(e, null)}
      >
        <ChevronRight
          size={12}
          className={`shrink-0 text-text-faint transition-transform ${isOpen ? "rotate-90" : ""}`}
        />
        <Layers size={12} className="shrink-0 text-text-muted" />
        <span className={showsActiveInline ? "shrink-0" : "truncate"}>{db.name}</span>
        {showsActiveInline && (
          <span className="truncate text-[11px] text-text-muted">
            · {activeCollection}
          </span>
        )}
      </button>
      {isOpen && (
        <div className="ml-4 border-l border-border-subtle/40 pl-2">
          {collectionsError ? (
            <p className="px-1.5 py-1 text-[11px] text-red-400">{collectionsError}</p>
          ) : collectionsLoading ? (
            <p className="px-1.5 py-1 text-[11px] text-text-faint">Loading…</p>
          ) : (
            collections.length === 0 && (
              <p className="px-1.5 py-1 text-[11px] text-text-faint">No collections</p>
            )
          )}
          {/* A search naming some of them narrows the list to those. */}
          {(matches ?? collections).map((coll) => (
            <button
              key={coll.name}
              type="button"
              className={`block w-full truncate px-1.5 py-1 text-left text-[11px] ${
                activeCollection === coll.name
                  ? `${activeRowClass} font-medium text-text-default`
                  : "text-text-muted hover:bg-sidebar-hover"
              }`}
              title={coll.name}
              onClick={() => openCollection(sessionId, connection, db.name, coll.name)}
              onContextMenu={(e) => openMenu(e, coll.name)}
            >
              {matches ? <Highlighted name={coll.name} query={query} /> : coll.name}
            </button>
          ))}
        </div>
      )}
      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          onClose={closeMenu}
          items={
            menu.collection === null
              ? [
                  {
                    label: `Open console on ${db.name}`,
                    onSelect: () => openConsole(connection, db.name, null),
                  },
                ]
              : [
                  {
                    label: "Open collection",
                    onSelect: () =>
                      openCollection(sessionId, connection, db.name, menu.collection!),
                  },
                  {
                    label: `Open console on ${db.name}`,
                    onSelect: () => openConsole(connection, db.name, menu.collection),
                  },
                ]
          }
        />
      )}
    </div>
  );
}
