import { create } from "zustand";
import { api } from "../lib/tauri";
import { useSessionsStore } from "./sessionsStore";
import type {
  ConnectionProfileInput,
  ConnectionProfileMeta,
  ConnectionTestResult,
  DatabaseInfo,
  SecretBackendInfo,
} from "../types/connection";

export interface ActiveSession {
  connectionId: string;
  sessionId: string;
  serverVersion: string | null;
  databases: DatabaseInfo[];
}

interface ConnectionsState {
  profiles: ConnectionProfileMeta[];
  /** Set once the list has loaded: an empty list before that means "unknown". */
  profilesLoaded: boolean;
  loading: boolean;
  error: string | null;
  /** Live sessions by connection id - any number of servers at once. */
  sessions: Record<string, ActiveSession>;
  /** Connections whose connect is in flight. */
  connecting: Record<string, true>;
  /** Why the last connect of a connection failed, shown on its row. */
  connectErrors: Record<string, string>;
  secretBackend: SecretBackendInfo | null;
  lastTestResult: ConnectionTestResult | null;

  refreshProfiles: () => Promise<void>;
  loadSecretBackendInfo: () => Promise<void>;
  saveProfile: (input: ConnectionProfileInput) => Promise<void>;
  deleteProfile: (id: string) => Promise<void>;
  testConnection: (input: ConnectionProfileInput) => Promise<void>;
  /** Opens a session for the connection, alongside any already open. */
  connect: (id: string) => Promise<void>;
  /** Ends the connection's session and closes its tabs. */
  disconnect: (id: string) => Promise<void>;
}

/** The live session id for a connection, read at call time. */
export function sessionIdFor(connectionId: string): string | null {
  return useConnectionsStore.getState().sessions[connectionId]?.sessionId ?? null;
}

function without<T>(record: Record<string, T>, key: string): Record<string, T> {
  const next = { ...record };
  delete next[key];
  return next;
}

export const useConnectionsStore = create<ConnectionsState>((set, get) => ({
  profiles: [],
  profilesLoaded: false,
  loading: false,
  error: null,
  sessions: {},
  connecting: {},
  connectErrors: {},
  secretBackend: null,
  lastTestResult: null,

  refreshProfiles: async () => {
    set({ loading: true, error: null });
    try {
      const profiles = await api.listConnectionProfiles();
      set({ profiles, profilesLoaded: true, loading: false });
    } catch (e) {
      set({ error: String(e), loading: false });
    }
  },

  loadSecretBackendInfo: async () => {
    try {
      const secretBackend = await api.secretBackendInfo();
      set({ secretBackend });
    } catch (e) {
      set({ error: String(e) });
    }
  },

  saveProfile: async (input) => {
    set({ loading: true, error: null });
    try {
      await api.saveConnectionProfile(input);
      await get().refreshProfiles();
    } catch (e) {
      set({ error: String(e), loading: false });
      throw e;
    }
  },

  deleteProfile: async (id) => {
    // Deleting the connection in use would leave a session nothing points to.
    if (get().sessions[id]) await get().disconnect(id);
    set({ loading: true, error: null });
    try {
      await api.deleteConnectionProfile(id);
      await get().refreshProfiles();
    } catch (e) {
      set({ error: String(e), loading: false });
    }
  },

  testConnection: async (input) => {
    set({ loading: true, error: null, lastTestResult: null });
    try {
      const result = await api.testConnection(input);
      set({ lastTestResult: result, loading: false });
    } catch (e) {
      set({ error: String(e), loading: false });
    }
  },

  connect: async (id) => {
    if (get().sessions[id] || get().connecting[id]) return;
    set((s) => ({
      connecting: { ...s.connecting, [id]: true },
      connectErrors: without(s.connectErrors, id),
    }));
    try {
      const handle = await api.connect(id);
      const databases = await api.listDatabases(handle.sessionId);
      set((s) => ({
        sessions: {
          ...s.sessions,
          [id]: {
            connectionId: id,
            sessionId: handle.sessionId,
            serverVersion: handle.serverVersion,
            databases,
          },
        },
      }));
    } catch (e) {
      set((s) => ({ connectErrors: { ...s.connectErrors, [id]: String(e) } }));
    } finally {
      set((s) => ({ connecting: without(s.connecting, id) }));
    }
  },

  disconnect: async (id) => {
    const session = get().sessions[id];
    if (!session) return;
    // Close its tabs first: nothing should run on a session that's going.
    useSessionsStore.getState().closeConnection(id);
    set((s) => ({ sessions: without(s.sessions, id) }));
    try {
      await api.disconnect(session.sessionId);
    } catch {
      // best-effort - the session is gone from the app either way
    }
  },
}));
