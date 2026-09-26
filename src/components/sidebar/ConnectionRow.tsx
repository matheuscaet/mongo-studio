import { useCallback, useEffect, useState } from "react";
import type { HTMLAttributes, MouseEvent } from "react";
import { ChevronRight, Database, Loader2, MoreHorizontal } from "lucide-react";
import { ask } from "@tauri-apps/plugin-dialog";
import { useConnectionsStore } from "../../store/connectionsStore";
import { DatabaseRow } from "./DatabaseRow";
import type { CollectionMatches } from "./useCollectionMatches";
import { databaseKey } from "../../store/sessionsStore";
import { ContextMenu } from "../ui/ContextMenu";
import type { ConnectionProfileMeta } from "../../types/connection";

interface ConnectionRowProps {
  profile: ConnectionProfileMeta;
  onEdit: (id: string) => void;
  /** Nesting depth in the folder tree. */
  indent?: number;
  /** Extra props for the row itself: drag handlers, data attributes. */
  rowProps?: HTMLAttributes<HTMLDivElement> & Record<`data-${string}`, string>;
  /** The sidebar search, lowercased. */
  query?: string;
  /** Collections matching the sidebar search, on any connection. */
  collectionMatches?: CollectionMatches | null;
}

export const INDENT_PX = 12;

export function ConnectionRow({
  profile,
  onEdit,
  indent = 0,
  rowProps,
  query = "",
  collectionMatches = null,
}: ConnectionRowProps) {
  const session = useConnectionsStore((s) => s.sessions[profile.id]);
  const connecting = useConnectionsStore((s) => s.connecting[profile.id] ?? false);
  const connectError = useConnectionsStore((s) => s.connectErrors[profile.id]);
  const connect = useConnectionsStore((s) => s.connect);
  const disconnect = useConnectionsStore((s) => s.disconnect);
  const deleteProfile = useConnectionsStore((s) => s.deleteProfile);
  const [expanded, setExpanded] = useState(false);
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  const closeMenu = useCallback(() => setMenu(null), []);

  const isActive = session !== undefined;
  // a search that found collections here shows them, even if collapsed
  const showChildren = expanded || (collectionMatches?.connectionIds.has(profile.id) ?? false);

  useEffect(() => {
    if (isActive) setExpanded(true);
  }, [isActive]);

  function openMenu(e: MouseEvent) {
    e.preventDefault();
    e.stopPropagation();
    setMenu({ x: e.clientX, y: e.clientY });
  }

  async function handleDelete() {
    const confirmed = await ask(
      `Delete the connection "${profile.name}"? Its saved passwords are removed too.`,
      { title: "Delete connection", kind: "warning", okLabel: "Delete", cancelLabel: "Cancel" },
    );
    if (confirmed) await deleteProfile(profile.id);
  }

  function handleToggle() {
    if (isActive) {
      setExpanded((e) => !e);
    } else {
      connect(profile.id);
    }
  }

  return (
    <div className={isActive ? "bg-sidebar-active" : ""}>
      <div
        {...rowProps}
        className={`group flex items-center gap-1 py-1.5 pr-2 text-sm ${
          isActive ? "hover:bg-sidebar-active-hover" : "hover:bg-sidebar-hover"
        } ${rowProps?.className ?? ""}`}
        style={{ paddingLeft: 8 + indent * INDENT_PX }}
        onContextMenu={openMenu}
      >
        <button
          type="button"
          className="flex min-w-0 flex-1 items-center gap-1.5 text-left"
          onClick={handleToggle}
        >
          <ChevronRight
            size={13}
            className={`shrink-0 text-text-faint transition-transform ${
              showChildren && isActive ? "rotate-90" : ""
            }`}
          />
          <Database size={14} className="shrink-0 text-text-muted" />
          <span className="truncate text-text-default">{profile.name}</span>
          {isActive && (
            <span className="ml-auto h-1.5 w-1.5 shrink-0 rounded-full bg-status-green" />
          )}
          {connecting && (
            <Loader2
              size={12}
              className="ml-auto shrink-0 animate-spin text-text-faint"
              aria-label="Connecting"
            />
          )}
        </button>
        <button
          type="button"
          className={`shrink-0 rounded text-text-faint hover:text-text-default focus:opacity-100 ${
            menu ? "opacity-100" : "opacity-0 group-hover:opacity-100"
          }`}
          onClick={openMenu}
          data-no-drag=""
          title="Connection actions"
          aria-label={`Actions for ${profile.name}`}
          aria-haspopup="menu"
        >
          <MoreHorizontal size={14} />
        </button>
      </div>
      {connectError && !isActive && (
        <p
          className="line-clamp-3 pb-1.5 pr-2 text-[11px] text-red-400"
          style={{ paddingLeft: 26 + indent * INDENT_PX }}
          title={connectError}
        >
          {connectError}
        </p>
      )}
      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          onClose={closeMenu}
          items={[
            isActive
              ? { label: "Disconnect", onSelect: () => disconnect(profile.id) }
              : { label: "Connect", onSelect: () => connect(profile.id) },
            { label: "Edit connection...", onSelect: () => onEdit(profile.id) },
            { label: "Delete connection...", onSelect: handleDelete },
          ]}
        />
      )}
      {isActive && showChildren && session && (
        <div
          className="border-l border-border-subtle/60 pl-2"
          style={{ marginLeft: 16 + indent * INDENT_PX }}
        >
          {session.databases.map((db) => (
            <DatabaseRow
              key={db.name}
              db={db}
              sessionId={session.sessionId}
              connection={{ id: profile.id, name: profile.name, summary: profile.summary }}
              query={query}
              matches={
                collectionMatches?.byDatabase.get(databaseKey(profile.id, db.name)) ?? null
              }
            />
          ))}
        </div>
      )}
    </div>
  );
}
