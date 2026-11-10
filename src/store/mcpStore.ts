import { create } from "zustand";
import { listen } from "@tauri-apps/api/event";
import { playChime, unlockAudioOnFirstGesture } from "../lib/attention";
import { api } from "../lib/tauri";
import type {
  McpActivity,
  McpCapability,
  McpClientTarget,
  McpConfig,
  McpConnectionChange,
  McpRequest,
  McpRequestChoice,
  McpStatus,
} from "../types/mcp";
import { useSettingsStore, type McpAlerts, type McpSettings } from "./settingsStore";
import { useConnectionsStore } from "./connectionsStore";
import { useScriptsStore } from "./scriptsStore";
import { useSidebarLayoutStore } from "./sidebarLayoutStore";

/** How many activity lines Settings keeps. */
const ACTIVITY_KEPT = 50;

interface McpState {
  status: McpStatus | null;
  /** The server's token, once fetched for Settings. */
  token: string | null;
  /** Calls set to Ask, oldest first, waiting for an answer. */
  requests: McpRequest[];
  activity: McpActivity[];
  targets: McpClientTarget[] | null;
  /** The client being added or removed. */
  registering: string | null;
  answer: (requestId: string, choice: McpRequestChoice) => Promise<void>;
  loadTargets: () => Promise<void>;
  setRegistered: (id: string, on: boolean) => Promise<void>;
}

export const useMcpStore = create<McpState>((set, get) => ({
  status: null,
  token: null,
  requests: [],
  activity: [],
  targets: null,
  registering: null,

  answer: async (requestId, choice) => {
    set((s) => ({ requests: s.requests.filter((r) => r.requestId !== requestId) }));
    await api.mcpAnswer(requestId, choice);
  },

  loadTargets: async () => {
    set({ targets: await api.mcpClientTargets() });
  },

  setRegistered: async (id, on) => {
    set({ registering: id });
    try {
      const target = on ? await api.mcpRegisterClient(id) : await api.mcpUnregisterClient(id);
      set({ targets: (get().targets ?? []).map((t) => (t.id === id ? target : t)) });
    } finally {
      set({ registering: null });
    }
  },
}));

function configOf(mcp: McpSettings, alerts: McpAlerts): McpConfig {
  return { enabled: mcp.enabled, port: mcp.port, caps: { ...mcp.caps, browse: "allow" }, connections: mcp.connections, always: mcp.always, alerts };
}

let started = false;

/**
 * Pushes the MCP settings to the backend now and whenever they change
 * ("saved as you change them"), and listens to the server's events. Called
 * once the app has mounted.
 */
export function startMcp() {
  if (started) return;
  started = true;
  const push = (mcp: McpSettings) =>
    api
      .mcpSetConfig(configOf(mcp, useSettingsStore.getState().mcpAlerts))
      .then((status) => useMcpStore.setState({ status }))
      .catch((e) =>
        useMcpStore.setState({ status: { running: false, port: mcp.port, url: "", error: String(e), clients: [] } }),
      );
  void push(useSettingsStore.getState().mcp);
  useSettingsStore.subscribe((s, prev) => {
    if (s.mcp !== prev.mcp || s.mcpAlerts !== prev.mcpAlerts) void push(s.mcp);
  });

  void listen<McpStatus>("mcp-status", ({ payload }) => useMcpStore.setState({ status: payload }));
  unlockAudioOnFirstGesture();
  void listen<McpRequest>("mcp-request", ({ payload }) => {
    useMcpStore.setState((s) => ({ requests: [...s.requests, payload] }));
    // the request waits for an answer; the backend brings the window forward
    if (useSettingsStore.getState().mcpAlerts.sound) void playChime();
  });
  void listen<{ requestId: string }>("mcp-request-closed", ({ payload }) =>
    useMcpStore.setState((s) => ({ requests: s.requests.filter((r) => r.requestId !== payload.requestId) })),
  );
  void listen<McpActivity>("mcp-activity", ({ payload }) => {
    useMcpStore.setState((s) => ({ activity: [payload, ...s.activity].slice(0, ACTIVITY_KEPT) }));
    if (payload.tool === "save_script" && payload.result === "ok") void useScriptsStore.getState().refresh();
  });
  // A connection a client created starts at Read; one it deleted loses its grants.
  void listen<McpConnectionChange>("mcp-connection", ({ payload }) => {
    const settings = useSettingsStore.getState();
    if (payload.change === "created") {
      settings.setMcpLevels([payload.connectionId], payload.level ?? "read");
      if (payload.folderId) {
        useSidebarLayoutStore.getState().placeConnections([payload.connectionId], { kind: "folder", id: payload.folderId });
      }
    } else if (payload.change === "deleted") {
      const { connections, always } = settings.mcp;
      const nextConnections = { ...connections };
      const nextAlways = { ...always };
      delete nextConnections[payload.connectionId];
      delete nextAlways[payload.connectionId];
      settings.setMcp({ connections: nextConnections, always: nextAlways });
    }
    void useConnectionsStore.getState().refreshProfiles();
  });
  void listen<{ connectionId: string; capability: McpCapability }>("mcp-always", ({ payload }) =>
    useSettingsStore.getState().addMcpAlways(payload.connectionId, payload.capability),
  );
}
