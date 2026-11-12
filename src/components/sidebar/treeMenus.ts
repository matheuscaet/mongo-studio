import { Copy, Download, FolderPlus, Link2, Pencil, Plug, SquareTerminal, Table, Trash2, Unplug } from "lucide-react";
import { toast } from "sonner";
import type { MenuEntry } from "@/components/common/ActionMenu";

/** Copies a name for pasting into a query or a script. */
function copyName(name: string) {
  navigator.clipboard.writeText(name).then(
    () => toast.success(`Copied ${name}`),
    (e) => toast.error("Couldn't copy the name", { description: String(e) }),
  );
}

interface FolderMenu {
  hasConnections: boolean;
  /** How many connections in the folder, at any depth, are connected. */
  connectedCount: number;
  onNewFolder: () => void;
  onRename: () => void;
  onDisconnectAll: () => void;
  onExport: () => void;
  onDeleteConnections: () => void;
  onDelete: () => void;
}

export function folderMenuEntries(m: FolderMenu): MenuEntry[] {
  return [
    { label: "New folder inside", icon: FolderPlus, onSelect: m.onNewFolder },
    { label: "Rename folder", icon: Pencil, onSelect: m.onRename },
    ...(m.hasConnections ? [{ label: "Export connections in folder...", icon: Download, onSelect: m.onExport }] : []),
    ...(m.connectedCount > 0
      ? [
          { separator: true } as const,
          {
            label: m.connectedCount === 1 ? "Disconnect 1 connection" : `Disconnect all ${m.connectedCount} connections`,
            icon: Unplug,
            onSelect: m.onDisconnectAll,
          },
        ]
      : []),
    { separator: true },
    ...(m.hasConnections
      ? [{ label: "Delete connections in folder...", icon: Trash2, danger: true, onSelect: m.onDeleteConnections }]
      : []),
    {
      label: m.hasConnections ? "Delete folder (move its connections out first)" : "Delete folder",
      icon: Trash2,
      danger: true,
      disabled: m.hasConnections,
      onSelect: m.onDelete,
    },
  ];
}

interface ConnectionMenu {
  connected: boolean;
  onConnect: () => void;
  onDisconnect: () => void;
  onEdit: () => void;
  onExport: () => void;
  onDelete: () => void;
}

export function connectionMenuEntries(m: ConnectionMenu): MenuEntry[] {
  return [
    m.connected
      ? { label: "Disconnect", icon: Unplug, onSelect: m.onDisconnect }
      : { label: "Connect", icon: Plug, onSelect: m.onConnect },
    { label: "Edit connection...", icon: Pencil, onSelect: m.onEdit },
    { label: "Export connection...", icon: Download, onSelect: m.onExport },
    { separator: true },
    { label: "Delete connection...", icon: Trash2, danger: true, onSelect: m.onDelete },
  ];
}

interface SelectionMenu {
  /** How many connections are picked. */
  count: number;
  /** How many of them are connected. */
  connectedCount: number;
  onDisconnect: () => void;
  onExport: () => void;
  onDelete: () => void;
}

/** The menu on a row among several picked connections: it works on all of them. */
export function selectionMenuEntries(m: SelectionMenu): MenuEntry[] {
  return [
    { heading: `${m.count} connections` },
    ...(m.connectedCount > 0
      ? [{ label: `Disconnect ${m.connectedCount === 1 ? "1 connection" : `${m.connectedCount} connections`}`, icon: Unplug, onSelect: m.onDisconnect }]
      : []),
    { label: `Export ${m.count} connections...`, icon: Download, onSelect: m.onExport },
    { separator: true },
    { label: `Delete ${m.count} connections...`, icon: Trash2, shortcut: "Del", danger: true, onSelect: m.onDelete },
  ];
}

interface DatabaseMenu {
  database: string;
  connectionName: string;
  onOpenConsole: () => void;
}

export function databaseMenuEntries(m: DatabaseMenu): MenuEntry[] {
  return [
    { heading: `${m.database} on ${m.connectionName}` },
    { label: `Open console on ${m.database}`, icon: SquareTerminal, onSelect: m.onOpenConsole },
    { label: "Copy name", icon: Copy, onSelect: () => copyName(m.database) },
  ];
}

interface CollectionMenu {
  database: string;
  collection: string;
  onOpen: () => void;
  onOpenConsole: () => void;
  onRelate: () => void;
}

export function collectionMenuEntries(m: CollectionMenu): MenuEntry[] {
  return [
    { heading: `${m.database}.${m.collection}` },
    { label: "Open collection", icon: Table, shortcut: "Enter", onSelect: m.onOpen },
    { label: `Open console on ${m.database}`, icon: SquareTerminal, onSelect: m.onOpenConsole },
    { label: "Relate to another collection…", icon: Link2, onSelect: m.onRelate },
    { label: "Copy name", icon: Copy, onSelect: () => copyName(m.collection) },
  ];
}
