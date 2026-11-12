import { useEffect, useState } from "react";
import type { HTMLAttributes } from "react";
import { confirm } from "../../store/confirmStore";
import { deleteConnections, useConnectionSelection } from "../../store/connectionSelectionStore";
import { useConnectionsStore } from "../../store/connectionsStore";
import { databaseKey } from "../../store/sessionsStore";
import { useUiStore } from "../../store/uiStore";
import type { ConnectionProfileMeta } from "../../types/connection";
import { RowContextMenu } from "@/components/common/ActionMenu";
import { Checkbox } from "@/components/ui/checkbox";
import { ConnectionChip } from "@/components/ui/ConnectionChip";
import { connectionColor } from "@/lib/connectionColor";
import { ConnectionStatus } from "./ConnectionStatus";
import { DatabaseRow } from "./DatabaseRow";
import { Highlighted } from "./Highlighted";
import { RowMenuButton } from "./RowMenuButton";
import { TreeRow } from "./TreeRow";
import { connectionMenuEntries, selectionMenuEntries } from "./treeMenus";
import type { CollectionMatches } from "./useCollectionMatches";

interface ConnectionRowProps {
  profile: ConnectionProfileMeta;
  /** Nesting depth in the folder tree. */
  depth?: number;
  /** Extra props for the row itself: drag handlers, data attributes. */
  rowProps?: HTMLAttributes<HTMLDivElement> & Record<`data-${string}`, string>;
  /** The sidebar search, lowercased. */
  query?: string;
  /** Collections matching the sidebar search, on any connection. */
  collectionMatches?: CollectionMatches | null;
  /** Picked in the Explorer's multi-selection. */
  selected?: boolean;
}

export function ConnectionRow({
  profile,
  depth = 0,
  rowProps,
  query = "",
  collectionMatches = null,
  selected = false,
}: ConnectionRowProps) {
  const session = useConnectionsStore((s) => s.sessions[profile.id]);
  const connecting = useConnectionsStore((s) => s.connecting[profile.id] ?? false);
  const connectError = useConnectionsStore((s) => s.connectErrors[profile.id]);
  const connect = useConnectionsStore((s) => s.connect);
  const disconnect = useConnectionsStore((s) => s.disconnect);
  const deleteProfile = useConnectionsStore((s) => s.deleteProfile);
  const selecting = useConnectionSelection((s) => s.selecting);
  const [expanded, setExpanded] = useState(false);

  const isActive = session !== undefined;
  // a search that found collections here shows them, even if collapsed
  const showChildren = expanded || (collectionMatches?.connectionIds.has(profile.id) ?? false);

  useEffect(() => {
    if (isActive) setExpanded(true);
  }, [isActive]);

  async function handleDelete() {
    const confirmed = await confirm({
      title: "Delete connection",
      message: `Delete the connection "${profile.name}"? Its saved passwords are removed too.`,
      confirmLabel: "Delete",
      danger: true,
    });
    if (confirmed) await deleteProfile(profile.id);
  }

  function handleToggle() {
    // in select mode a click (or Enter / Space) picks the row instead
    if (selecting) {
      useConnectionSelection.getState().toggle(profile.id);
    } else if (isActive) {
      setExpanded((e) => !e);
    } else {
      connect(profile.id);
    }
  }

  const entries = () => {
    // on a row among several picked, the menu works on all of them
    const picked = useConnectionSelection.getState().ids;
    if (picked.length > 1 && picked.includes(profile.id)) {
      const { sessions, disconnectAll } = useConnectionsStore.getState();
      return selectionMenuEntries({
        count: picked.length,
        connectedCount: picked.filter((id) => sessions[id]).length,
        onDisconnect: () => void disconnectAll(picked),
        onExport: () => useUiStore.getState().openTransfer("export", picked),
        onDelete: () => void deleteConnections(picked),
      });
    }
    return connectionMenuEntries({
      connected: isActive,
      onConnect: () => connect(profile.id),
      onDisconnect: () => disconnect(profile.id),
      onEdit: () => useUiStore.getState().setConnectionDialog({ mode: "edit", id: profile.id }),
      onExport: () => useUiStore.getState().openTransfer("export", [profile.id]),
      onDelete: handleDelete,
    });
  };

  return (
    <>
      <RowContextMenu entries={entries}>
        <TreeRow
          {...rowProps}
          selected={selected}
          depth={depth}
          expanded={isActive ? showChildren : undefined}
          icon={
            <>
              {selecting && (
                // the row takes the click; the box only shows the state
                <Checkbox checked={selected} tabIndex={-1} aria-hidden className="pointer-events-none" />
              )}
              <ConnectionChip name={profile.name} color={connectionColor(profile.id, profile.color)} />
            </>
          }
          labelClassName="font-medium"
          label={<Highlighted name={profile.name} query={query} />}
          trailing={
            <>
              <ConnectionStatus
                name={profile.name}
                connected={isActive}
                connecting={connecting}
                serverVersion={session?.serverVersion ?? null}
                error={isActive ? undefined : connectError}
                onRetry={() => connect(profile.id)}
              />
              <RowMenuButton entries={entries} label={`Actions for ${profile.name}`} />
            </>
          }
          onActivate={handleToggle}
        />
      </RowContextMenu>
      {isActive && showChildren && (
        <div role="group">
          {session.databases.map((db) => (
            <DatabaseRow
              key={db.name}
              db={db}
              sessionId={session.sessionId}
              connection={{ id: profile.id, name: profile.name, summary: profile.summary }}
              depth={depth + 1}
              query={query}
              matches={
                collectionMatches?.byDatabase.get(databaseKey(profile.id, db.name)) ?? null
              }
            />
          ))}
        </div>
      )}
    </>
  );
}
