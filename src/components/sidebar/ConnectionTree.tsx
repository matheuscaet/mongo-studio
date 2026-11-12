import { useCallback, useMemo } from "react";
import type { KeyboardEvent, MouseEvent } from "react";
import { deleteConnections, useConnectionSelection } from "../../store/connectionSelectionStore";
import { useConnectionsStore } from "../../store/connectionsStore";
import { useSidebarLayoutStore } from "../../store/sidebarLayoutStore";
import { filterTree } from "../../lib/sidebarTree";
import type { DropTarget, TreeNode } from "../../lib/sidebarTree";
import type { ConnectionProfileMeta } from "../../types/connection";
import { cn } from "@/lib/utils";
import { ConnectionRow } from "./ConnectionRow";
import { FolderRow } from "./FolderRow";
import { useTreeDrag } from "./useTreeDrag";
import { useCollectionMatches } from "./useCollectionMatches";
import type { DragState } from "./useTreeDrag";

const LINE_ABOVE = "shadow-[inset_0_2px_0_var(--color-accent)]";
const LINE_BELOW = "shadow-[inset_0_-2px_0_var(--color-accent)]";

/** Drop feedback on a row: a line above or below it, or a highlight into it. */
function indicatorClass(drag: DragState | null, nodeId: string): string {
  if (!drag) return "";
  if (drag.nodeId === nodeId) return "opacity-40";
  const ind = drag.indicator;
  if (!ind || ind.nodeId !== nodeId) return "";
  if (ind.position === "before") return LINE_ABOVE;
  if (ind.position === "after") return LINE_BELOW;
  return "bg-accent/20 outline outline-1 -outline-offset-1 outline-accent hover:bg-accent/20";
}

/** The connections a row's shown for, top to bottom: what a Shift-click ranges over. */
function visibleConnections(nodes: TreeNode[], searching: boolean): string[] {
  return nodes.flatMap((n) =>
    n.type === "connection"
      ? [n.id]
      : searching || !n.collapsed
        ? visibleConnections(n.children, searching)
        : [],
  );
}

interface ConnectionTreeProps {
  search: string;
}

/** Connections arranged in folders, rearranged by dragging. */
export function ConnectionTree({ search }: ConnectionTreeProps) {
  const profiles = useConnectionsStore((s) => s.profiles);
  const root = useSidebarLayoutStore((s) => s.root);
  const move = useSidebarLayoutStore((s) => s.move);
  const getRoot = useCallback(() => useSidebarLayoutStore.getState().root, []);
  const onDrop = useCallback((id: string, target: DropTarget) => move(id, target), [move]);
  const { drag, startPress } = useTreeDrag(getRoot, onDrop);

  const byId = useMemo(() => new Map(profiles.map((p) => [p.id, p])), [profiles]);
  const query = search.trim().toLowerCase();
  const searching = query !== "";
  const collectionMatches = useCollectionMatches(query);
  // A connection stays in view when its name matches, or when collections
  // listed under it do.
  const shown = searching
    ? filterTree(
        root,
        (id) =>
          (collectionMatches?.connectionIds.has(id) ?? false) ||
          (byId.get(id)?.name.toLowerCase().includes(query) ?? false),
      )
    : root;
  const selectedIds = useConnectionSelection((s) => s.ids);
  const selected = useMemo(() => new Set(selectedIds), [selectedIds]);

  // Cmd/Ctrl-click picks a connection and Shift-click a run of them, without
  // connecting; a plain click clears the picks and works the row as ever.
  function handleRowClick(e: MouseEvent, id: string) {
    const selection = useConnectionSelection.getState();
    if (e.metaKey || e.ctrlKey) {
      e.preventDefault();
      selection.toggle(id);
    } else if (e.shiftKey) {
      e.preventDefault();
      selection.selectRange(id, visibleConnections(shown, searching));
    } else if (selection.ids.length > 0 && !selection.selecting) {
      selection.clear();
    }
  }

  function handleKeyDown(e: KeyboardEvent<HTMLDivElement>) {
    // keys typed in a field inside the tree (a folder's name) are its own
    if (e.target instanceof HTMLInputElement) return;
    const { ids, selecting, setSelecting } = useConnectionSelection.getState();
    if (e.key === "Escape" && (ids.length > 0 || selecting)) {
      e.preventDefault();
      setSelecting(false);
    } else if (ids.length > 0 && (e.key === "Delete" || e.key === "Backspace")) {
      e.preventDefault();
      void deleteConnections(ids);
    }
  }

  function renderNodes(nodes: TreeNode[], depth: number) {
    return nodes.map((node) => {
      if (node.type === "folder") {
        // a search shows every folder on a hit's path, however it was left
        const expanded = searching || !node.collapsed;
        return (
          <div key={node.id} role="group">
            <FolderRow
              folder={node}
              depth={depth}
              expanded={expanded}
              dragClass={indicatorClass(drag, node.id)}
              onPress={startPress}
            />
            {expanded && renderNodes(node.children, depth + 1)}
          </div>
        );
      }
      const profile: ConnectionProfileMeta | undefined = byId.get(node.id);
      if (!profile) return null;
      return (
        <ConnectionRow
          key={node.id}
          profile={profile}
          depth={depth}
          query={query}
          collectionMatches={collectionMatches}
          selected={selected.has(node.id)}
          rowProps={{
            "data-node-id": node.id,
            "data-node-type": "connection",
            className: indicatorClass(drag, node.id),
            onPointerDown: (e) => startPress(e, node.id, profile.name),
            onClick: (e) => handleRowClick(e, node.id),
          }}
        />
      );
    });
  }

  const endIndicator = drag?.indicator?.position === "end";

  return (
    <div
      data-tree-root=""
      role="tree"
      aria-label="Connections"
      aria-multiselectable
      className="flex min-h-full flex-col pb-3 pt-0.5"
      onKeyDown={handleKeyDown}
    >
      {searching && shown.length === 0 && (
        <p className="px-3 py-2.5 text-sm text-fg-3">
          No connection or open collection matches “{search.trim()}”.
        </p>
      )}
      {renderNodes(shown, 0)}
      {/* Room below the last row to drop things back at the top level. */}
      <div className={cn("min-h-6 flex-1", endIndicator && LINE_ABOVE)} />
      {drag && (
        <div
          className="pointer-events-none fixed z-[70] flex h-6 items-center rounded-md border border-accent bg-popover px-2 text-sm text-fg shadow-overlay"
          style={{ left: drag.x + 12, top: drag.y + 8 }}
        >
          {drag.label}
        </div>
      )}
    </div>
  );
}
