import { create } from "zustand";
import { toast } from "sonner";
import { documentKey } from "../../lib/bsonFormat";
import { isPlainObject } from "../../lib/bsonValue";
import { parseQueryObject } from "../../lib/queryText";
import { api } from "../../lib/tauri";
import { useConnectionsStore } from "../../store/connectionsStore";
import { TabKind, useSessionsStore, type CollectionTab } from "../../store/sessionsStore";
import { clearPicks } from "./useDocumentSelection";

/** What the delete dialog asks about. */
export type DeleteRequest =
  | {
      kind: "documents";
      tabId: string;
      /** The documents, each with its _id. */
      documents: Record<string, unknown>[];
    }
  | {
      kind: "filter";
      tabId: string;
      /** The filter text that produced the results, as typed. */
      filterText: string;
      filter: Record<string, unknown>;
      /** How many documents it matches, once counted. */
      count: number | null;
      countError: string | null;
    };

interface DeleteState {
  pending: DeleteRequest | null;
  running: boolean;
  error: string | null;
}

export const useDeleteStore = create<DeleteState>(() => ({ pending: null, running: false, error: null }));

function collectionTab(tabId: string): CollectionTab | null {
  const tab = useSessionsStore.getState().tabs.find((t) => t.id === tabId);
  return tab?.kind === TabKind.Collection ? tab : null;
}

function sessionIdOf(tab: CollectionTab): string | null {
  return useConnectionsStore.getState().sessions[tab.connection.id]?.sessionId ?? null;
}

/**
 * Whether a tab's documents can be deleted from its results: like in-place
 * edits, only a find's documents are the collection's as stored, and only
 * while the connection is open.
 */
export function canDeleteFrom(tab: CollectionTab | null | undefined): boolean {
  return !!tab && tab.resultsMode === "find" && !!tab.results && sessionIdOf(tab) !== null;
}

/** Opens the dialog to delete these documents of a tab's results. */
export function askDeleteDocuments(tabId: string, documents: unknown[]) {
  const tab = collectionTab(tabId);
  if (!canDeleteFrom(tab)) return;
  const withId = documents.filter((d): d is Record<string, unknown> => isPlainObject(d) && d._id !== undefined);
  if (withId.length === 0) {
    toast.error("Nothing to delete", { description: "These documents have no _id to delete them by." });
    return;
  }
  useDeleteStore.setState({ pending: { kind: "documents", tabId, documents: withId }, running: false, error: null });
}

/** Opens the dialog to delete every document the tab's last find matched, and counts them. */
export function askDeleteMatching(tabId: string) {
  const tab = collectionTab(tabId);
  const sessionId = tab && sessionIdOf(tab);
  if (!tab || !sessionId || !canDeleteFrom(tab)) return;
  const filterText = tab.resultsFilter ?? tab.filterText;
  let filter: Record<string, unknown>;
  try {
    filter = parseQueryObject(filterText, "filter");
  } catch (e) {
    toast.error("The filter can't be read", { description: String(e) });
    return;
  }
  const request: DeleteRequest = { kind: "filter", tabId, filterText, filter, count: null, countError: null };
  useDeleteStore.setState({ pending: request, running: false, error: null });
  api
    .countDocuments(sessionId, tab.database, tab.collection, filter)
    .then((count) => {
      if (useDeleteStore.getState().pending === request) {
        useDeleteStore.setState({ pending: { ...request, count } });
      }
    })
    .catch((e) => {
      if (useDeleteStore.getState().pending === request) {
        useDeleteStore.setState({ pending: { ...request, countError: String(e) } });
      }
    });
}

export function closeDeleteDialog() {
  if (!useDeleteStore.getState().running) useDeleteStore.setState({ pending: null, error: null });
}

const plural = (n: number) => `${n.toLocaleString("en")} ${n === 1 ? "document" : "documents"}`;

/** Deletes what the dialog asked about, then takes it out of the results. */
export async function confirmDelete() {
  const { pending, running } = useDeleteStore.getState();
  if (!pending || running) return;
  const tab = collectionTab(pending.tabId);
  const sessionId = tab && sessionIdOf(tab);
  if (!tab || !sessionId) {
    useDeleteStore.setState({ error: "The connection is closed. Connect again and retry." });
    return;
  }
  const ns = `${tab.database}.${tab.collection}`;
  useDeleteStore.setState({ running: true, error: null });
  try {
    if (pending.kind === "documents") {
      const ids = pending.documents.map((d) => d._id);
      const deleted = await api.deleteDocuments(sessionId, tab.database, tab.collection, ids);
      const keys = pending.documents.map(documentKey).filter((k): k is string => k !== null);
      useSessionsStore.getState().removeDocuments(tab.id, keys, deleted);
      const missing = ids.length - deleted;
      toast.success(`${plural(deleted)} deleted`, {
        description:
          missing > 0
            ? `${ns} · ${plural(missing)} had already been deleted`
            : `${ids.length === 1 ? "deleteOne" : "deleteMany"} on ${ns}`,
      });
    } else {
      const deleted = await api.deleteMatching(sessionId, tab.database, tab.collection, pending.filter);
      // every document on the page matched the filter
      const keys = (tab.results?.documents ?? []).map(documentKey).filter((k): k is string => k !== null);
      useSessionsStore.getState().removeDocuments(tab.id, keys, deleted);
      toast.success(`${plural(deleted)} deleted`, { description: `deleteMany on ${ns}` });
    }
    clearPicks(tab.id);
    useDeleteStore.setState({ pending: null, running: false });
  } catch (e) {
    useDeleteStore.setState({ running: false, error: String(e) });
  }
}

/** Copies documents as Extended JSON: one object, or an array of them. */
export async function copyDocuments(documents: unknown[]) {
  const value = documents.length === 1 ? documents[0] : documents;
  try {
    await navigator.clipboard.writeText(JSON.stringify(value, null, 2));
    toast.success(documents.length === 1 ? "Copied the document" : `Copied ${documents.length} documents`, {
      description: "As Extended JSON",
    });
  } catch {
    toast.error("The clipboard isn't available");
  }
}
