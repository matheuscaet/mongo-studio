import { create } from "zustand";
import { persist } from "zustand/middleware";
import type { McpCapability, McpLevel, McpPolicy } from "../types/mcp";

/** Which engine runs console scripts. */
export type ScriptEngine = "builtin" | "mongosh";

/** How mongosh is started, see Settings › Console. */
export interface MongoshSettings {
  /** An executable picked by hand; null uses the one found. */
  path: string | null;
  extraArgs: string;
  loadRc: boolean;
}

/** The MCP server's settings, pushed to the backend as they change. */
export interface McpSettings {
  enabled: boolean;
  port: number;
  caps: Record<Exclude<McpCapability, "browse">, McpPolicy>;
  /** Per saved connection id; one missing here has no access. */
  connections: Record<string, McpLevel>;
  /** Capabilities answered "Always allow here", per connection id. */
  always: Record<string, McpCapability[]>;
}

/** How Mongo Studio calls for attention when an MCP request waits. */
export interface McpAlerts {
  /** A short chime. */
  sound: boolean;
  /** The window comes to the front (or its taskbar entry flashes). */
  focus: boolean;
}

export const DEFAULT_MCP_PORT = 47321;

export const DEFAULT_MCP_CAPS: McpSettings["caps"] = {
  read: "allow",
  write: "ask",
  ddl: "off",
  script: "ask",
  connList: "allow",
  connEdit: "ask",
  connDelete: "off",
  saveScript: "allow",
  relRead: "allow",
  relEdit: "allow",
};

export const PAGE_SIZES = [20, 50, 100, 200] as const;

interface SettingsState {
  /** Put the last session's tabs back on launch, dormant until opened. */
  reopenTabs: boolean;
  setReopenTabs: (on: boolean) => void;
  /** How many documents a new tab's Find bar asks for. */
  pageSize: number;
  setPageSize: (size: number) => void;

  /** The engine new consoles start with. */
  consoleEngine: ScriptEngine;
  setConsoleEngine: (engine: ScriptEngine) => void;
  /** A console switched from its toolbar, by tab id; not kept across launches. */
  engineByTab: Record<string, ScriptEngine>;
  setTabEngine: (tabId: string, engine: ScriptEngine) => void;
  forgetTabEngine: (tabId: string) => void;
  mongosh: MongoshSettings;
  setMongosh: (patch: Partial<MongoshSettings>) => void;

  mcp: McpSettings;
  setMcp: (patch: Partial<McpSettings>) => void;
  /** What happens when an MCP request set to Ask waits for an answer. */
  mcpAlerts: McpAlerts;
  setMcpAlerts: (patch: Partial<McpAlerts>) => void;
  setMcpCap: (cap: keyof McpSettings["caps"], policy: McpPolicy) => void;
  setMcpLevels: (connectionIds: string[], level: McpLevel) => void;
  addMcpAlways: (connectionId: string, cap: McpCapability) => void;
  removeMcpAlways: (connectionId: string, cap: McpCapability) => void;
}

export const useSettingsStore = create<SettingsState>()(
  persist(
    (set) => ({
      reopenTabs: true,
      setReopenTabs: (reopenTabs) => set({ reopenTabs }),
      pageSize: 50,
      setPageSize: (pageSize) => set({ pageSize }),

      consoleEngine: "builtin",
      setConsoleEngine: (consoleEngine) => set({ consoleEngine }),
      engineByTab: {},
      setTabEngine: (tabId, engine) => set((s) => ({ engineByTab: { ...s.engineByTab, [tabId]: engine } })),
      forgetTabEngine: (tabId) =>
        set((s) => {
          if (!(tabId in s.engineByTab)) return s;
          const engineByTab = { ...s.engineByTab };
          delete engineByTab[tabId];
          return { engineByTab };
        }),
      mongosh: { path: null, extraArgs: "", loadRc: false },
      setMongosh: (patch) => set((s) => ({ mongosh: { ...s.mongosh, ...patch } })),

      mcp: {
        enabled: false,
        port: DEFAULT_MCP_PORT,
        caps: DEFAULT_MCP_CAPS,
        connections: {},
        always: {},
      },
      setMcp: (patch) => set((s) => ({ mcp: { ...s.mcp, ...patch } })),
      mcpAlerts: { sound: true, focus: true },
      setMcpAlerts: (patch) => set((s) => ({ mcpAlerts: { ...s.mcpAlerts, ...patch } })),
      setMcpCap: (cap, policy) => set((s) => ({ mcp: { ...s.mcp, caps: { ...s.mcp.caps, [cap]: policy } } })),
      setMcpLevels: (ids, level) =>
        set((s) => {
          const connections = { ...s.mcp.connections };
          for (const id of ids) connections[id] = level;
          return { mcp: { ...s.mcp, connections } };
        }),
      addMcpAlways: (connectionId, cap) =>
        set((s) => {
          const caps = s.mcp.always[connectionId] ?? [];
          if (caps.includes(cap)) return s;
          return { mcp: { ...s.mcp, always: { ...s.mcp.always, [connectionId]: [...caps, cap] } } };
        }),
      removeMcpAlways: (connectionId, cap) =>
        set((s) => {
          const caps = (s.mcp.always[connectionId] ?? []).filter((c) => c !== cap);
          const always = { ...s.mcp.always };
          if (caps.length) always[connectionId] = caps;
          else delete always[connectionId];
          return { mcp: { ...s.mcp, always } };
        }),
    }),
    {
      name: "mongo-studio-settings",
      version: 1,
      partialize: (s) => ({
        reopenTabs: s.reopenTabs,
        pageSize: s.pageSize,
        consoleEngine: s.consoleEngine,
        mongosh: s.mongosh,
        mcp: s.mcp,
        mcpAlerts: s.mcpAlerts,
      }),
      // Capabilities added later get their defaults.
      merge: (persisted, current) => {
        const p = (persisted ?? {}) as Partial<SettingsState>;
        return {
          ...current,
          ...p,
          mongosh: { ...current.mongosh, ...p.mongosh },
          mcpAlerts: { ...current.mcpAlerts, ...p.mcpAlerts },
          mcp: { ...current.mcp, ...p.mcp, caps: { ...DEFAULT_MCP_CAPS, ...p.mcp?.caps } },
        };
      },
    },
  ),
);

/** The engine a console runs with, read now (for callbacks). */
export function engineFor(tabId: string): ScriptEngine {
  const s = useSettingsStore.getState();
  return s.engineByTab[tabId] ?? s.consoleEngine;
}

/** The engine a console runs with: its own, or the default. */
export function useTabEngine(tabId: string): ScriptEngine {
  return useSettingsStore((s) => s.engineByTab[tabId] ?? s.consoleEngine);
}
