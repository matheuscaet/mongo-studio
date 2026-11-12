import { create } from "zustand";
import { persist } from "zustand/middleware";

export type MainTab = "browse" | "indexes" | "console";

/**
 * How query and script results are rendered: a document grid with the
 * selected document in an inspector beside it, a JSON tree, or raw JSON.
 */
export type ResultView = "grid" | "tree" | "json";

/** Which tool window the side panel shows, or null when it's hidden. */
export type SidePanel = "explorer" | "scripts" | "relations" | null;

/** The connection dialog: a new connection, or editing a saved one. */
export type ConnectionDialog = { mode: "new" } | { mode: "edit"; id: string } | null;

/** The import / export dialog: which side, and the connections an export starts with. */
export type TransferDialog = { tab: "export" | "import"; ids: string[] | null };

/** Settings' sections. */
export type SettingsSection = "general" | "results" | "console" | "connections" | "assistant" | "mcp";

/** The console's editor and output: side by side, or editor above output. */
export type ConsoleLayout = "side" | "stacked";

interface UiState {
  /** Each collection tab's view (Documents, Indexes, Console); unset is Documents. */
  mainTabs: Record<string, MainTab>;
  setMainTab: (tabId: string, view: MainTab) => void;
  /**
   * Each result pane's view, by `resultViewKey`; a pane with none uses
   * `defaultResultView`. Panes switch on their own, never together.
   */
  resultViews: Record<string, ResultView>;
  setResultView: (key: string, view: ResultView) => void;
  /** Drops the views of a closed tab's panes. */
  forgetResultViews: (tabId: string) => void;
  /** The view new tabs and consoles start in, set in Settings. */
  defaultResultView: ResultView;
  setDefaultResultView: (view: ResultView) => void;
  consoleLayout: ConsoleLayout;
  setConsoleLayout: (layout: ConsoleLayout) => void;
  /** The editor's share of the console, remembered per layout. */
  consoleSplit: Record<ConsoleLayout, number>;
  setConsoleSplit: (layout: ConsoleLayout, ratio: number) => void;
  /** Sidebar width in px. */
  sidebarWidth: number;
  setSidebarWidth: (width: number) => void;
  sidePanel: SidePanel;
  /** Shows a tool window; showing the one already open hides the panel. */
  toggleSidePanel: (panel: Exclude<SidePanel, null>) => void;
  setSidePanel: (panel: SidePanel) => void;
  /** The inspector beside the document grid, in px. */
  inspectorWidth: number;
  setInspectorWidth: (width: number) => void;
  connectionDialog: ConnectionDialog;
  setConnectionDialog: (dialog: ConnectionDialog) => void;
  /** The import / export connections dialog. */
  transfer: TransferDialog | null;
  /** Opens it on `tab`; an export starts with `ids` ticked (all when null). */
  openTransfer: (tab: TransferDialog["tab"], ids?: string[] | null) => void;
  closeTransfer: () => void;
  /** The delete connections dialog: the connections it starts with ticked. */
  deleteConnections: string[] | null;
  /** Opens it with `ids` ticked (none when omitted). */
  openDeleteConnections: (ids?: string[]) => void;
  closeDeleteConnections: () => void;
  /** The Settings dialog's section, or null when it's closed. */
  settingsSection: SettingsSection | null;
  setSettingsSection: (section: SettingsSection | null) => void;
  /** The Ctrl+K quick-open palette. */
  paletteOpen: boolean;
  setPaletteOpen: (open: boolean) => void;
}

export const DEFAULT_CONSOLE_SPLIT = 0.6;
export const DEFAULT_SIDEBAR_WIDTH = 280;
export const DEFAULT_INSPECTOR_WIDTH = 380;

export const useUiStore = create<UiState>()(
  persist(
    (set) => ({
      mainTabs: {},
      setMainTab: (tabId, view) => set((s) => ({ mainTabs: { ...s.mainTabs, [tabId]: view } })),
      resultViews: {},
      setResultView: (key, view) => set((s) => ({ resultViews: { ...s.resultViews, [key]: view } })),
      forgetResultViews: (tabId) =>
        set((s) => {
          const keys = Object.keys(s.resultViews).filter((k) => k === tabId || k.startsWith(`${tabId}#`));
          if (keys.length === 0) return s;
          const resultViews = { ...s.resultViews };
          for (const k of keys) delete resultViews[k];
          return { resultViews };
        }),
      defaultResultView: "grid",
      setDefaultResultView: (view) => set({ defaultResultView: view }),
      consoleLayout: "stacked",
      setConsoleLayout: (layout) => set({ consoleLayout: layout }),
      consoleSplit: { side: DEFAULT_CONSOLE_SPLIT, stacked: DEFAULT_CONSOLE_SPLIT },
      setConsoleSplit: (layout, ratio) =>
        set((s) => ({ consoleSplit: { ...s.consoleSplit, [layout]: ratio } })),
      sidebarWidth: DEFAULT_SIDEBAR_WIDTH,
      setSidebarWidth: (width) => set({ sidebarWidth: width }),
      sidePanel: "explorer",
      toggleSidePanel: (panel) => set((s) => ({ sidePanel: s.sidePanel === panel ? null : panel })),
      setSidePanel: (panel) => set({ sidePanel: panel }),
      inspectorWidth: DEFAULT_INSPECTOR_WIDTH,
      setInspectorWidth: (width) => set({ inspectorWidth: width }),
      connectionDialog: null,
      setConnectionDialog: (dialog) => set({ connectionDialog: dialog }),
      transfer: null,
      openTransfer: (tab, ids = null) => set({ transfer: { tab, ids }, settingsSection: null }),
      closeTransfer: () => set({ transfer: null }),
      deleteConnections: null,
      openDeleteConnections: (ids = []) => set({ deleteConnections: ids, settingsSection: null }),
      closeDeleteConnections: () => set({ deleteConnections: null }),
      settingsSection: null,
      setSettingsSection: (section) => set({ settingsSection: section }),
      paletteOpen: false,
      setPaletteOpen: (open) => set({ paletteOpen: open }),
    }),
    {
      name: "mongo-studio-ui",
      // Only layout sizes are lasting preferences; which tab is open resets
      // on launch as it always has.
      partialize: (s) => ({
        consoleLayout: s.consoleLayout,
        consoleSplit: s.consoleSplit,
        sidebarWidth: s.sidebarWidth,
        inspectorWidth: s.inspectorWidth,
        defaultResultView: s.defaultResultView,
      }),
      version: 2,
      migrate: (state) => {
        // v1 kept one view for every pane; it becomes the default for new
        // ones. Views from before the redesign: "table" became the grid.
        const s = state as { resultView?: string; defaultResultView?: string };
        const view = s.defaultResultView ?? s.resultView;
        s.defaultResultView = view === "tree" || view === "json" ? view : "grid";
        delete s.resultView;
        return s as never;
      },
    },
  ),
);

/** The key of a collection tab's results pane, or of a console's output. */
export function resultViewKey(tabId: string, pane: "documents" | "console"): string {
  return pane === "documents" ? tabId : `${tabId}#console`;
}

/** A result pane's view: its own, or the default until it's switched. */
export function useResultView(key: string): ResultView {
  return useUiStore((s) => s.resultViews[key] ?? s.defaultResultView);
}
