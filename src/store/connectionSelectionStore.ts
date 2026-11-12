import { create } from "zustand";
import { toast } from "sonner";
import { confirm } from "./confirmStore";
import { useConnectionsStore } from "./connectionsStore";

interface ConnectionSelectionState {
  /** Connections picked in the Explorer, in the order they were picked. */
  ids: string[];
  /** Where a Shift-click range starts: the last connection clicked. */
  anchor: string | null;
  /** Select mode: rows show checkboxes, and clicking one picks it instead of connecting. */
  selecting: boolean;
  /** Turns select mode on or off; off clears the picks. */
  setSelecting: (on: boolean) => void;
  /** Picks or un-picks all of `ids`, e.g. a folder's connections. */
  setMany: (ids: string[], on: boolean) => void;
  /** Cmd/Ctrl-click: adds the connection, or takes it out. */
  toggle: (id: string) => void;
  /** Shift-click: picks every connection from the anchor to `id`, as `order` lists them. */
  selectRange: (id: string, order: string[]) => void;
  clear: () => void;
  set: (ids: string[]) => void;
}

/** The connections from `anchor` to `id`, both included, in `order`'s order. */
export function rangeBetween(order: string[], anchor: string | null, id: string): string[] {
  const end = order.indexOf(id);
  if (end < 0) return [];
  const start = anchor === null ? -1 : order.indexOf(anchor);
  // no anchor, or one that's gone out of view: the range is just the row
  if (start < 0) return [id];
  return order.slice(Math.min(start, end), Math.max(start, end) + 1);
}

/**
 * The Explorer's multi-selection of connections, for acting on several at
 * once. Outside select mode, a plain click on a row clears it.
 */
export const useConnectionSelection = create<ConnectionSelectionState>((set, get) => ({
  ids: [],
  anchor: null,
  selecting: false,
  setSelecting: (on) => set(on ? { selecting: true } : { selecting: false, ids: [], anchor: null }),
  setMany: (ids, on) =>
    set((s) => ({
      ids: on ? [...s.ids, ...ids.filter((id) => !s.ids.includes(id))] : s.ids.filter((id) => !ids.includes(id)),
    })),
  toggle: (id) =>
    set((s) => ({
      ids: s.ids.includes(id) ? s.ids.filter((x) => x !== id) : [...s.ids, id],
      anchor: id,
    })),
  selectRange: (id, order) => set({ ids: rangeBetween(order, get().anchor, id), anchor: get().anchor ?? id }),
  clear: () => set({ ids: [], anchor: null }),
  set: (ids) => set({ ids, anchor: ids[ids.length - 1] ?? null }),
}));

const listNames = (names: string[]) =>
  names.length <= 3
    ? names.map((n) => `"${n}"`).join(", ")
    : `${names.slice(0, 3).map((n) => `"${n}"`).join(", ")} and ${names.length - 3} more`;

/** Asks, then deletes these connections; the ones that fail stay selected. */
export async function deleteConnections(ids: string[]): Promise<void> {
  const { profiles, sessions, deleteProfiles } = useConnectionsStore.getState();
  const picked = profiles.filter((p) => ids.includes(p.id));
  if (picked.length === 0) return;
  const connected = picked.filter((p) => sessions[p.id]).length;
  const count = picked.length === 1 ? "the connection" : `${picked.length} connections`;
  const confirmed = await confirm({
    title: picked.length === 1 ? "Delete connection" : `Delete ${picked.length} connections`,
    message:
      `Delete ${count} ${listNames(picked.map((p) => p.name))}? Their saved passwords are removed too.` +
      (connected > 0 ? ` ${connected} connected ${connected === 1 ? "is" : "are"} disconnected first.` : ""),
    confirmLabel: "Delete",
    danger: true,
  });
  if (!confirmed) return;
  const failed = await deleteProfiles(picked.map((p) => p.id));
  useConnectionSelection.getState().set(failed);
  if (failed.length > 0) {
    toast.error(`Couldn't delete ${failed.length === 1 ? "1 connection" : `${failed.length} connections`}`, {
      description: useConnectionsStore.getState().error ?? undefined,
    });
  }
}
