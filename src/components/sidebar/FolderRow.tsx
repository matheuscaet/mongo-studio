import { useEffect, useRef } from "react";
import type { PointerEvent } from "react";
import { Folder, FolderOpen } from "lucide-react";
import { useConnectionSelection } from "../../store/connectionSelectionStore";
import { useConnectionsStore } from "../../store/connectionsStore";
import { useSidebarLayoutStore } from "../../store/sidebarLayoutStore";
import { useUiStore } from "../../store/uiStore";
import { connectionIds, containsConnections } from "../../lib/sidebarTree";
import type { FolderNode } from "../../lib/sidebarTree";
import { RowContextMenu } from "@/components/common/ActionMenu";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";
import { RowMenuButton } from "./RowMenuButton";
import { groupState } from "../connections/ConnectionTreePicker";
import { TreeRow } from "./TreeRow";
import { folderMenuEntries } from "./treeMenus";

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
  /** Drop feedback classes from the drag in progress. */
  dragClass: string;
  onPress: (e: PointerEvent, id: string, label: string) => void;
}

/** A folder of connections: toggles on click, renames inline. */
export function FolderRow({ folder, depth, expanded, dragClass, onPress }: FolderRowProps) {
  const renaming = useSidebarLayoutStore((s) => s.renamingId === folder.id);
  const { toggleFolder, startRename, renameFolder, deleteFolder, createFolder } =
    useSidebarLayoutStore.getState();
  const inputRef = useRef<HTMLInputElement>(null);
  const selecting = useConnectionSelection((s) => s.selecting);
  const pickedIds = useConnectionSelection((s) => s.ids);

  useEffect(() => {
    if (renaming) inputRef.current?.select();
  }, [renaming]);

  // built when a menu opens, so "holds connections" and what's connected are current
  const entries = () => {
    const { sessions, disconnectAll } = useConnectionsStore.getState();
    const connected = connectionIds(folder.children).filter((id) => sessions[id]);
    return folderMenuEntries({
      hasConnections: containsConnections(folder),
      connectedCount: connected.length,
      onNewFolder: () => createFolder(folder.id),
      onRename: () => startRename(folder.id),
      onDisconnectAll: () => void disconnectAll(connected),
      onExport: () => useUiStore.getState().openTransfer("export", connectionIds(folder.children)),
      onDeleteConnections: () =>
        useUiStore.getState().openDeleteConnections(connectionIds(folder.children)),
      onDelete: () => deleteFolder(folder.id),
    });
  };

  const Icon = expanded ? FolderOpen : Folder;
  const count = countConnections(folder);
  const ids = selecting ? connectionIds(folder.children) : [];
  const folderState = groupState(ids, (id) => pickedIds.includes(id));

  return (
    <RowContextMenu entries={entries}>
      <TreeRow
        data-node-id={folder.id}
        data-node-type="folder"
        data-collapsed={expanded ? "false" : "true"}
        depth={depth}
        expanded={expanded}
        className={cn(dragClass)}
        icon={
          <>
            {selecting && ids.length > 0 && (
              // ticks everything in the folder; the rest of the row still opens it
              <Checkbox
                data-no-drag=""
                checked={folderState}
                aria-label={`All in ${folder.name}`}
                onClick={(e) => e.stopPropagation()}
                onCheckedChange={() => useConnectionSelection.getState().setMany(ids, folderState !== true)}
              />
            )}
            <Icon className="size-3.5 shrink-0 text-fg-2" aria-hidden />
          </>
        }
        labelClassName={renaming ? "flex-1" : undefined}
        label={
          renaming ? (
            <Input
              ref={inputRef}
              data-no-drag=""
              aria-label="Folder name"
              defaultValue={folder.name}
              className="h-5 rounded-xs px-1 text-base"
              autoFocus
              onClick={(e) => e.stopPropagation()}
              onDoubleClick={(e) => e.stopPropagation()}
              onKeyDown={(e) => {
                e.stopPropagation();
                if (e.key === "Enter") renameFolder(folder.id, e.currentTarget.value);
                if (e.key === "Escape") startRename(null);
              }}
              onBlur={(e) => renameFolder(folder.id, e.currentTarget.value)}
            />
          ) : (
            folder.name
          )
        }
        trailing={
          !renaming && (
            <>
              {count > 0 && <span>{count}</span>}
              <RowMenuButton entries={entries} label={`Actions for folder ${folder.name}`} />
            </>
          )
        }
        onPointerDown={(e) => onPress(e, folder.id, folder.name)}
        onActivate={() => !renaming && toggleFolder(folder.id)}
        onDoubleClick={(e) => {
          e.stopPropagation();
          startRename(folder.id);
        }}
      />
    </RowContextMenu>
  );
}
