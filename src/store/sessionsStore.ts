import { create } from "zustand";
import { api } from "../lib/tauri";
import { useUiStore } from "./uiStore";
import { forgetRelatedPages } from "./relatedStore";
import { useSettingsStore } from "./settingsStore";
import { parseOptionalQueryObject, parseQueryArray, parseQueryObject } from "../lib/queryText";
import type { CollectionInfo } from "../types/connection";
import type { CollectionStats, QueryResultPage } from "../types/query";

export type QueryMode = "find" | "aggregate";

/** The connection a tab was opened on, as shown on the tab. */
export interface TabConnection {
  id: string;
  name: string;
  /** Server address with credentials masked, e.g. mongodb://***@host:27017. */
  summary: string;
}

/** One open collection, with its own query, results and stats. */
/** What a tab holds. The values are what saved sessions store, so they stay as they are. */
export enum TabKind {
  Collection = "collection",
  Console = "console",
  Relation = "relation",
}

export interface CollectionTab {
  kind: TabKind.Collection;
  id: string;
  connection: TabConnection;
  database: string;
  collection: string;
  stats: CollectionStats | null;
  results: QueryResultPage | null;
  /**
   * The mode that produced `results`. Only find results are documents as
   * stored; aggregate output can be regrouped or computed, so it isn't
   * editable even when it has an _id.
   */
  resultsMode: QueryMode | null;
  /** How long the query behind `results` took, round trip included, in ms. */
  queryMs: number | null;
  mode: QueryMode;
  filterText: string;
  sortText: string;
  limit: number;
  skip: number;
  pipelineText: string;
  loading: boolean;
  /** The running query's execution id, for Cancel; null when none runs. */
  runId?: string | null;
  error: string | null;
  /** The query field the Assistant last wrote, until the user edits it. */
  assistantSource?: "filter" | "pipeline" | null;
  /**
   * Reopened from the last session and not connected yet: it connects, and
   * runs its query, when it's opened.
   */
  dormant?: boolean;
}

/**
 * A script console on its own, bound to one database of one connection.
 * Scripts can reach any collection of that database.
 */
export interface ConsoleTab {
  kind: TabKind.Console;
  id: string;
  connection: TabConnection;
  database: string;
  /** The collection it was opened from, which its first script queries. */
  collection: string | null;
  /** Numbers consoles on the same database apart: 1, 2, ... */
  number: number;
  /** Opened with a script the Assistant wrote. */
  fromAssistant?: boolean;
  /** Reopened from the last session and not connected yet. */
  dormant?: boolean;
}

/**
 * A relation's editor. `connection` is the connection its sample documents
 * come from, so it closes with that connection like any other tab.
 */
export interface RelationTab {
  kind: TabKind.Relation;
  id: string;
  connection: TabConnection;
  /** The relation's From database, where its samples are read. */
  database: string;
  /** No collection of its own: code that targets "the tab's collection" sees none. */
  collection: null;
  relationId: string;
  dormant?: boolean;
}

export type Tab = CollectionTab | ConsoleTab | RelationTab;

/** The part of a tab the query bar edits. */
export type TabQueryFields = Pick<
  CollectionTab,
  "mode" | "filterText" | "sortText" | "limit" | "skip" | "pipelineText"
>;

/** A database's row in the sidebar: open or not, and its collections. */
export interface DatabaseTreeState {
  expanded: boolean;
  collections: CollectionInfo[];
  /** Whether `collections` has been fetched; reopening reuses it. */
  loaded: boolean;
  loading: boolean;
  error: string | null;
}

/** A database on a connection. */
export interface DatabaseRef {
  connectionId: string;
  database: string;
}

interface SessionsState {
  /**
   * Every database row's state, by `databaseKey`. Purely visual:
   * collapsing a database leaves every tab open on it alone.
   */
  databaseTree: Record<string, DatabaseTreeState>;
  /** The database last opened in the sidebar. */
  lastDatabase: DatabaseRef | null;
  tabs: Tab[];
  activeTabId: string | null;

  toggleDatabase: (connectionId: string, sessionId: string, database: string) => Promise<void>;
  /** Focuses the collection's tab, opening one if it has none yet. */
  openCollection: (
    sessionId: string,
    connection: TabConnection,
    database: string,
    collection: string,
  ) => Promise<void>;
  /** Focuses the relation's editor tab, opening it on the connection its samples come from. */
  openRelation: (relationId: string, connection: TabConnection, database: string) => void;
  /**
   * Points a console tab at another database, as `use <db>` does - on its
   * own connection, or on `connection` to move it to another server.
   */
  switchConsoleDatabase: (id: string, database: string, connection?: TabConnection) => void;
  /** Opens a new console on the database, however many it already has; returns its tab id. */
  openConsole: (
    connection: TabConnection,
    database: string,
    collection: string | null,
    options?: { fromAssistant?: boolean },
  ) => string;
  activateTab: (id: string) => void;
  closeTab: (id: string) => void;
  closeOtherTabs: (id: string) => void;
  closeAllTabs: () => void;
  updateTab: (id: string, patch: Partial<TabQueryFields>) => void;
  /** Puts a query the Assistant wrote into the tab, marked as its. */
  applyQuery: (id: string, patch: Partial<TabQueryFields>, source: "filter" | "pipeline") => void;
  runQuery: (sessionId: string, id: string) => Promise<void>;
  /** Stops the tab's running query; its last results stay. */
  cancelQuery: (id: string) => void;
  /** Swaps in a document as stored after an edit, matched on _id. */
  replaceDocument: (id: string, updated: unknown) => void;
  /** Forgets a connection going away: its tabs and its sidebar state. */
  closeConnection: (connectionId: string) => void;
  /** Puts back the last session's tabs, dormant, before any tab is open. */
  restoreTabs: (tabs: Tab[], activeTabId: string | null) => void;
  /** A dormant tab's connection is live: it runs (collection) or just shows (console). */
  wakeTab: (sessionId: string, id: string) => Promise<void>;
}

/**
 * Unique per collection per connection: database names can't contain dots,
 * and the connection id keeps same-named collections on different servers
 * apart.
 */
export function tabIdFor(connectionId: string, database: string, collection: string): string {
  return `${connectionId}/${database}.${collection}`;
}

export function selectActiveTab(state: SessionsState): Tab | null {
  return state.tabs.find((t) => t.id === state.activeTabId) ?? null;
}

/** Numbers consoles on one database apart: one past the highest open. */
function nextConsoleNumber(tabs: Tab[], connectionId: string, database: string): number {
  const numbers = tabs
    .filter((t) => t.kind === TabKind.Console && t.connection.id === connectionId && t.database === database)
    .map((t) => (t as ConsoleTab).number);
  return Math.max(0, ...numbers) + 1;
}

/** Key of a database in `databaseTree`: database names can't contain "/". */
export function databaseKey(connectionId: string, database: string): string {
  return `${connectionId}/${database}`;
}

/**
 * Newest first, as people usually want to look. Not for time series: they
 * have no `_id` index, so on a large one the sort would run in memory and
 * can fail.
 */
function defaultSort(collectionType: string | undefined): string {
  return collectionType === "timeseries" ? "" : "{ _id: -1 }";
}

function newTab(
  connection: TabConnection,
  database: string,
  collection: string,
  collectionType?: string,
): CollectionTab {
  // Settings' "Documents per page".
  const pageSize = useSettingsStore.getState().pageSize;
  return {
    kind: TabKind.Collection,
    id: tabIdFor(connection.id, database, collection),
    connection,
    database,
    collection,
    stats: null,
    results: null,
    resultsMode: null,
    queryMs: null,
    mode: "find",
    filterText: "{}",
    sortText: defaultSort(collectionType),
    limit: pageSize,
    skip: 0,
    pipelineText: `[\n  { "$limit": ${pageSize} }\n]`,
    loading: false,
    error: null,
  };
}

const initialState = {
  databaseTree: {} as Record<string, DatabaseTreeState>,
  lastDatabase: null as DatabaseRef | null,
  tabs: [] as Tab[],
  activeTabId: null as string | null,
};

const closedDatabase: DatabaseTreeState = {
  expanded: false,
  collections: [],
  loaded: false,
  loading: false,
  error: null,
};

export const useSessionsStore = create<SessionsState>((set, get) => {
  // Requests outlive the tab that made them when it's closed mid-flight;
  // patching a tab that's gone is simply a no-op.
  function patchTab(id: string, patch: Partial<Omit<CollectionTab, "kind">>) {
    set((s) => ({
      tabs: s.tabs.map((t) => (t.id === id && t.kind === TabKind.Collection ? { ...t, ...patch } : t)),
    }));
  }

  function collectionTab(id: string): CollectionTab | null {
    const tab = get().tabs.find((t) => t.id === id);
    return tab?.kind === TabKind.Collection ? tab : null;
  }

  return {
    ...initialState,

    toggleDatabase: async (connectionId, sessionId, database) => {
      const key = databaseKey(connectionId, database);
      const patchDatabase = (patch: Partial<DatabaseTreeState>) =>
        set((s) => ({
          databaseTree: {
            ...s.databaseTree,
            [key]: { ...(s.databaseTree[key] ?? closedDatabase), ...patch },
          },
        }));
      const current = get().databaseTree[key] ?? closedDatabase;
      if (current.expanded) {
        patchDatabase({ expanded: false });
        return;
      }
      set({ lastDatabase: { connectionId, database } });
      // Reopening a database already listed: show its collections again.
      if (current.loaded || current.loading) {
        patchDatabase({ expanded: true });
        return;
      }
      patchDatabase({ expanded: true, loading: true, error: null });
      try {
        const collections = await api.listCollections(sessionId, database);
        // Gone if the connection closed meanwhile.
        if (get().databaseTree[key]) patchDatabase({ collections, loaded: true, loading: false });
      } catch (e) {
        // Not loaded, so the next open retries.
        if (get().databaseTree[key]) patchDatabase({ error: String(e), loading: false });
      }
    },

    openCollection: async (sessionId, connection, database, collection) => {
      const id = tabIdFor(connection.id, database, collection);
      if (get().tabs.some((t) => t.id === id)) {
        set({ activeTabId: id });
        return;
      }
      const collectionType = get()
        .databaseTree[databaseKey(connection.id, database)]?.collections.find((c) => c.name === collection)
        ?.collectionType;
      set((s) => ({
        tabs: [...s.tabs, { ...newTab(connection, database, collection, collectionType), loading: true }],
        activeTabId: id,
      }));
      // The documents come first. The stats only feed the count in the
      // header and status bar, so they load alongside and a failure there
      // leaves the results alone.
      api
        .getCollectionStats(sessionId, database, collection)
        .then((stats) => patchTab(id, { stats }))
        .catch(() => {});
      await get().runQuery(sessionId, id);
    },

    openConsole: (connection, database, collection, options) => {
      const tab: ConsoleTab = {
        kind: TabKind.Console,
        id: `console:${crypto.randomUUID()}`,
        connection,
        database,
        collection,
        number: nextConsoleNumber(get().tabs, connection.id, database),
        fromAssistant: options?.fromAssistant ?? false,
      };
      set((s) => ({ tabs: [...s.tabs, tab], activeTabId: tab.id }));
      return tab.id;
    },

    openRelation: (relationId, connection, database) => {
      const id = `relation:${relationId}`;
      set((s) => {
        const existing = s.tabs.find((t) => t.id === id);
        // reopened on another connection: the samples follow it
        const tab: RelationTab = { kind: TabKind.Relation, id, connection, database, collection: null, relationId };
        return {
          tabs: existing ? s.tabs.map((t) => (t.id === id ? tab : t)) : [...s.tabs, tab],
          activeTabId: id,
        };
      });
    },

    switchConsoleDatabase: (id, database, connection) =>
      set((s) => ({
        tabs: s.tabs.map((t) => {
          if (t.id !== id || t.kind !== TabKind.Console) return t;
          const target = connection ?? t.connection;
          if (target.id === t.connection.id && t.database === database) return t;
          return {
            ...t,
            connection: target,
            database,
            // the collection it was opened from belongs to the old database
            collection: null,
            number: nextConsoleNumber(s.tabs, target.id, database),
          };
        }),
      })),

    activateTab: (id) => set({ activeTabId: id }),

    closeTab: (id) =>
      set((s) => {
        const index = s.tabs.findIndex((t) => t.id === id);
        if (index === -1) return s;
        const tabs = s.tabs.filter((t) => t.id !== id);
        // Closing the active tab hands focus to its right neighbour, or the
        // left one when it was last - the way editor tabs behave.
        const activeTabId =
          s.activeTabId === id
            ? (tabs[index] ?? tabs[index - 1])?.id ?? null
            : s.activeTabId;
        return { tabs, activeTabId };
      }),

    closeOtherTabs: (id) =>
      set((s) =>
        s.tabs.some((t) => t.id === id)
          ? { tabs: s.tabs.filter((t) => t.id === id), activeTabId: id }
          : s,
      ),

    closeAllTabs: () => set({ tabs: [], activeTabId: null }),

    updateTab: (id, patch) => {
      const tab = collectionTab(id);
      // An edit of the field the Assistant wrote makes it the user's again.
      const edited =
        (tab?.assistantSource === "filter" && patch.filterText !== undefined && patch.filterText !== tab.filterText) ||
        (tab?.assistantSource === "pipeline" && patch.pipelineText !== undefined && patch.pipelineText !== tab.pipelineText);
      patchTab(id, edited ? { ...patch, assistantSource: null } : patch);
    },

    applyQuery: (id, patch, source) => patchTab(id, { ...patch, assistantSource: source }),

    runQuery: async (sessionId, id) => {
      const tab = collectionTab(id);
      if (!tab) return;
      // running again replaces a query still going
      if (tab.runId) void api.cancelQuery(tab.runId);
      const runId = crypto.randomUUID();
      // a cancelled or superseded run must not overwrite what came after it
      const current = () => collectionTab(id)?.runId === runId;
      patchTab(id, { loading: true, error: null, runId });
      const started = performance.now();
      const elapsed = () => Math.round(performance.now() - started);
      try {
        // The fields keep the text as typed (mongosh style or JSON); only
        // the Extended JSON it reads as goes to the backend.
        if (tab.mode === "aggregate") {
          const pipeline = parseQueryArray(tab.pipelineText);
          const results = await api.runAggregate(
            sessionId,
            tab.database,
            tab.collection,
            pipeline,
            runId,
          );
          if (current()) {
            patchTab(id, { results, resultsMode: "aggregate", queryMs: elapsed(), loading: false, runId: null });
          }
        } else {
          const filter = parseQueryObject(tab.filterText, "filter");
          const sort = parseOptionalQueryObject(tab.sortText, "sort");
          const results = await api.runFind(sessionId, tab.database, tab.collection, {
            filter,
            sort,
            projection: null,
            limit: tab.limit,
            skip: tab.skip,
          }, runId);
          if (current()) {
            patchTab(id, { results, resultsMode: "find", queryMs: elapsed(), loading: false, runId: null });
          }
        }
      } catch (e) {
        if (current()) patchTab(id, { error: String(e), loading: false, runId: null });
      }
    },

    cancelQuery: (id) => {
      const tab = collectionTab(id);
      if (!tab?.runId) return;
      void api.cancelQuery(tab.runId);
      // the results stay as they were, and say they're not this query's
      patchTab(id, {
        loading: false,
        runId: null,
        error: tab.results ? "Cancelled. The documents below are from the previous run." : "Cancelled.",
      });
    },

    replaceDocument: (id, updated) => {
      const tab = collectionTab(id);
      if (!tab?.results) return;
      const key = JSON.stringify((updated as { _id?: unknown } | null)?._id);
      patchTab(id, {
        results: {
          ...tab.results,
          documents: tab.results.documents.map((d) =>
            JSON.stringify((d as { _id?: unknown } | null)?._id) === key ? updated : d,
          ),
        },
      });
    },

    restoreTabs: (tabs, activeTabId) =>
      set((s) =>
        s.tabs.length > 0
          ? s
          : { tabs, activeTabId: tabs.some((t) => t.id === activeTabId) ? activeTabId : (tabs[0]?.id ?? null) },
      ),

    wakeTab: async (sessionId, id) => {
      const tab = get().tabs.find((t) => t.id === id);
      if (!tab?.dormant) return;
      set((s) => ({ tabs: s.tabs.map((t) => (t.id === id ? { ...t, dormant: false } : t)) }));
      if (tab.kind !== TabKind.Collection) return;
      api
        .getCollectionStats(sessionId, tab.database, tab.collection)
        .then((stats) => patchTab(id, { stats }))
        .catch(() => {});
      await get().runQuery(sessionId, id);
    },

    closeConnection: (connectionId) =>
      set((s) => {
        const prefix = databaseKey(connectionId, "");
        const tabs = s.tabs.filter((t) => t.connection.id !== connectionId);
        const activeGone = !tabs.some((t) => t.id === s.activeTabId);
        return {
          tabs,
          activeTabId: activeGone ? (tabs[tabs.length - 1]?.id ?? null) : s.activeTabId,
          databaseTree: Object.fromEntries(
            Object.entries(s.databaseTree).filter(([key]) => !key.startsWith(prefix)),
          ),
          lastDatabase:
            s.lastDatabase?.connectionId === connectionId ? null : s.lastDatabase,
        };
      }),
  };
});

// A closed tab's result views and related documents go with it, so the
// records don't grow.
useSessionsStore.subscribe((state, prev) => {
  if (state.tabs === prev.tabs) return;
  const open = new Set(state.tabs.map((t) => t.id));
  for (const tab of prev.tabs) {
    if (open.has(tab.id)) continue;
    useUiStore.getState().forgetResultViews(tab.id);
    forgetRelatedPages(tab.id);
  }
});
