import { create } from "zustand";
import { listen } from "@tauri-apps/api/event";
import { api } from "../lib/tauri";
import { useConnectionsStore } from "./connectionsStore";
import { selectActiveTab, useSessionsStore } from "./sessionsStore";
import type { ScriptLogEvent } from "../types/script";

/** The script a console starts with, pointed at its collection if it has one. */
export function defaultScript(database: string, collection: string | null): string {
  const header = `// db.collection("name") gives you find/findOne/insertOne/updateOne/deleteOne/aggregate/countDocuments.
// Top-level await is supported.`;
  if (collection) {
    return `// Console for ${database}.${collection} - runs against the "${database}" database.
${header}
const docs = await db.collection(${JSON.stringify(collection)}).find({}, { limit: 20 });
docs;
`;
  }
  return `// Console for the "${database}" database - any of its collections.
${header}
// e.g. await db.collection("orders").find({}, { limit: 20 });
`;
}

/**
 * The console on screen - the active tab's, whether a console tab or a
 * collection's own - with its script, the default one until edited. Null
 * without a tab. Read from the stores at call time, so it suits callbacks
 * bound once, like Monaco commands.
 */
export function currentConsoleTarget() {
  const tab = selectActiveTab(useSessionsStore.getState());
  if (!tab) return null;
  const session = useConnectionsStore.getState().sessions[tab.connection.id] ?? null;
  const untouched = defaultScript(tab.database, tab.collection);
  const stored = useConsoleStore.getState().consoles[tab.id]?.script;
  return {
    tab,
    session,
    database: tab.database,
    key: tab.id,
    untouched,
    script: stored ?? untouched,
  };
}

export interface ConsoleSession {
  /** Null until edited: the console shows its default script until then. */
  script: string | null;
  running: boolean;
  executionId: string | null;
  logs: string[];
  result: unknown;
  hasResult: boolean;
  error: string | null;
}

const blankSession: ConsoleSession = {
  script: null,
  running: false,
  executionId: null,
  logs: [],
  result: null,
  hasResult: false,
  error: null,
};

interface ConsoleState {
  /** One console per tab, keyed by tab id. */
  consoles: Record<string, ConsoleSession>;

  setScript: (key: string, script: string) => void;
  run: (key: string, sessionId: string, database: string, script: string) => Promise<void>;
  cancel: (key: string) => Promise<void>;
}

export const useConsoleStore = create<ConsoleState>((set, get) => {
  function patch(key: string, changes: Partial<ConsoleSession>) {
    set((s) => ({
      consoles: {
        ...s.consoles,
        [key]: { ...(s.consoles[key] ?? blankSession), ...changes },
      },
    }));
  }

  return {
    consoles: {},

    setScript: (key, script) => patch(key, { script }),

    run: async (key, sessionId, database, script) => {
      const executionId = crypto.randomUUID();
      patch(key, {
        script,
        running: true,
        executionId,
        logs: [],
        result: null,
        hasResult: false,
        error: null,
      });
      // A run whose console was closed, or re-run, meanwhile is dropped.
      const stillCurrent = () => get().consoles[key]?.executionId === executionId;
      try {
        const result = await api.runScript(sessionId, database, script, executionId, null);
        if (stillCurrent()) {
          patch(key, { running: false, result: result.value, hasResult: true, logs: result.logs });
        }
      } catch (e) {
        if (stillCurrent()) patch(key, { running: false, error: String(e) });
      }
    },

    cancel: async (key) => {
      const executionId = get().consoles[key]?.executionId;
      if (!executionId) return;
      await api.cancelScript(executionId);
    },
  };
});

// A tab's console goes when the tab does, including when a disconnect
// closes them all.
useSessionsStore.subscribe((state, prev) => {
  if (state.tabs === prev.tabs) return;
  const open = new Set(state.tabs.map((t) => t.id));
  const { consoles } = useConsoleStore.getState();
  const stale = Object.keys(consoles).filter((k) => !open.has(k));
  if (stale.length === 0) return;
  const kept = { ...consoles };
  for (const key of stale) delete kept[key];
  useConsoleStore.setState({ consoles: kept });
});

listen<ScriptLogEvent>("script-log", (event) => {
  const { consoles } = useConsoleStore.getState();
  const key = Object.keys(consoles).find(
    (k) => consoles[k].executionId === event.payload.executionId,
  );
  if (key === undefined) return;
  useConsoleStore.setState({
    consoles: {
      ...consoles,
      [key]: { ...consoles[key], logs: [...consoles[key].logs, event.payload.message] },
    },
  });
});
