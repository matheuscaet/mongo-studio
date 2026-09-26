import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { MouseEvent, PointerEvent } from "react";
import { ChevronRight, Folder, FolderOpen, MoreHorizontal } from "lucide-react";
import { useConnectionsStore } from "../../store/connectionsStore";
import { useSidebarLayoutStore } from "../../store/sidebarLayoutStore";
import { containsConnections, filterTree } from "../../lib/sidebarTree";
import type { DropTarget, FolderNode, TreeNode } from "../../lib/sidebarTree";
import { ContextMenu } from "../ui/ContextMenu";
import { ConnectionRow, INDENT_PX } from "./ConnectionRow";
import { useTreeDrag } from "./useTreeDrag";
import { useCollectionMatches } from "./useCollectionMatches";
import type { DragState } from "./useTreeDrag";
import type { ConnectionProfileMeta } from "../../types/connection";

/** Drop feedback on a row: a line above or below it, or a highlight into it. */
function indicatorClass(drag: DragState | null, nodeId: string): string {
  if (!drag) return "";
  if (drag.nodeId === nodeId) return "opacity-40";
  const ind = drag.indicator;
  if (!ind || ind.nodeId !== nodeId) return "";
  if (ind.position === "before") return "shadow-[inset_0_2px_0_var(--color-accent)]";
  if (ind.position === "after") return "shadow-[inset_0_-2px_0_var(--color-accent)]";
  return "bg-accent/25";
}

function countConnections(folder: FolderNode): number {
  return folder.children.reduce(
    (n, c) => n + (c.type === "connection" ? 1 : countConnections(c)),
    0,
  );
}

interface FolderRowProps {
  folder: FolderNode;
  depth: number;
  expanded: boolean;
  drag: DragState | null;
  onPress: (e: PointerEvent, id: string, label: string) => void;
}

function FolderRow({ folder, depth, expanded, drag, onPress }: FolderRowProps) {
  const renaming = useSidebarLayoutStore((s) => s.renamingId === folder.id);
  const { toggleFolder, startRename, renameFolder, deleteFolder, createFolder } =
    useSidebarLayoutStore.getState();
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  const closeMenu = useCallback(() => setMenu(null), []);
  const inputRef = useRef<HTMLInputElement>(null);
  const hasConnections = containsConnections(folder);

  useEffect(() => {
    if (renaming) inputRef.current?.select();
  }, [renaming]);

  function openMenu(e: MouseEvent) {
    e.preventDefault();
    e.stopPropagation();
    setMenu({ x: e.clientX, y: e.clientY });
  }

  const Icon = expanded ? FolderOpen : Folder;

  return (
    <>
      <div
        data-node-id={folder.id}
        data-node-type="folder"
        data-collapsed={expanded ? "false" : "true"}
        role="treeitem"
        aria-expanded={expanded}
        className={`group flex cursor-pointer items-center gap-1 py-1.5 pr-2 text-sm hover:bg-sidebar-hover ${indicatorClass(
          drag,
          folder.id,
        )}`}
        style={{ paddingLeft: 8 + depth * INDENT_PX }}
        onPointerDown={(e) => onPress(e, folder.id, folder.name)}
        onClick={() => !renaming && toggleFolder(folder.id)}
        onDoubleClick={(e) => {
          e.stopPropagation();
          startRename(folder.id);
        }}
        onContextMenu={openMenu}
      >
        <ChevronRight
          size={13}
          className={`shrink-0 text-text-faint transition-transform ${expanded ? "rotate-90" : ""}`}
        />
        <Icon size={14} className="shrink-0 text-text-muted" />
        {renaming ? (
          <input
            ref={inputRef}
            data-no-drag=""
            aria-label="Folder name"
            defaultValue={folder.name}
            className="min-w-0 flex-1 rounded border border-accent bg-editor px-1 text-sm text-text-default focus:outline-none"
            autoFocus
            onClick={(e) => e.stopPropagation()}
            onKeyDown={(e) => {
              e.stopPropagation();
              if (e.key === "Enter") renameFolder(folder.id, e.currentTarget.value);
              if (e.key === "Escape") startRename(null);
            }}
            onBlur={(e) => renameFolder(folder.id, e.currentTarget.value)}
          />
        ) : (
          <span className="truncate text-text-default">{folder.name}</span>
        )}
        {!renaming && (
          <span className="ml-auto shrink-0 text-[11px] text-text-faint">
            {countConnections(folder) || ""}
          </span>
        )}
        <button
          type="button"
          data-no-drag=""
          className={`shrink-0 rounded text-text-faint hover:text-text-default focus:opacity-100 ${
            menu ? "opacity-100" : "opacity-0 group-hover:opacity-100"
          }`}
          onClick={openMenu}
          title="Folder actions"
          aria-label={`Actions for folder ${folder.name}`}
          aria-haspopup="menu"
        >
          <MoreHorizontal size={14} />
        </button>
      </div>
      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          onClose={closeMenu}
          items={[
            { label: "New folder inside", onSelect: () => createFolder(folder.id) },
            { label: "Rename folder", onSelect: () => startRename(folder.id) },
            {
              label: hasConnections
                ? "Delete folder (move its connections out first)"
                : "Delete folder",
              onSelect: () => deleteFolder(folder.id),
              disabled: hasConnections,
            },
          ]}
        />
      )}
    </>
  );
}

interface ConnectionTreeProps {
  search: string;
  onEdit: (id: string) => void;
}

/** Connections arranged in folders, rearranged by dragging. */
export function ConnectionTree({ search, onEdit }: ConnectionTreeProps) {
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
              drag={drag}
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
          onEdit={onEdit}
          indent={depth}
          query={query}
          collectionMatches={collectionMatches}
          rowProps={{
            "data-node-id": node.id,
            "data-node-type": "connection",
            className: indicatorClass(drag, node.id),
            onPointerDown: (e) => startPress(e, node.id, profile.name),
          }}
        />
      );
    });
  }

  const endIndicator = drag?.indicator?.position === "end";

  return (
    <div data-tree-root="" role="tree" className="flex min-h-full flex-col pb-2">
      {searching && shown.length === 0 && (
        <p className="px-3 py-2 text-xs text-text-faint">No matches.</p>
      )}
      {renderNodes(shown, 0)}
      {/* Room below the last row to drop things back at the top level. */}
      <div
        className={`min-h-6 flex-1 ${endIndicator ? "shadow-[inset_0_2px_0_var(--color-accent)]" : ""}`}
      />
      {drag && (
        <div
          className="pointer-events-none fixed z-[70] rounded border border-accent bg-panel-alt px-2 py-0.5 text-xs text-text-default shadow-lg"
          style={{ left: drag.x + 12, top: drag.y + 8 }}
        >
          {drag.label}
        </div>
      )}
    </div>
  );
}
