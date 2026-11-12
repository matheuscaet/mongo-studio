import {
  ArrowLeftRight,
  Database,
  FolderOpen,
  Link2,
  Moon,
  PanelRight,
  Plus,
  Server,
  Settings,
  SlidersHorizontal,
  Sparkles,
  SquareTerminal,
  Sun,
  Trash2,
} from "lucide-react";
import { toast } from "sonner";
import { useAssistantStore } from "../../store/assistantStore";
import type { ReactNode } from "react";
import { CommandGroup, CommandItem, CommandShortcut } from "@/components/ui/command";
import { api } from "../../lib/tauri";
import { SPLIT_LAYOUT_ORDER, SPLIT_LAYOUTS } from "../../lib/editorLayout";
import { useEditorLayoutStore } from "../../store/editorLayoutStore";
import { selectActiveTab, TabKind, useSessionsStore } from "../../store/sessionsStore";
import { LayoutIcon } from "../tabs/LayoutPicker";
import { useThemeStore } from "../../store/themeStore";
import { useUiStore } from "../../store/uiStore";
import { HighlightMatch } from "./HighlightMatch";
import { newRelationFromContext } from "../relations/RelationsPanel";

interface PaletteActionsProps {
  search: string;
  /** Closes the palette, then runs the choice. */
  run: (action: () => void) => void;
}

interface PaletteAction {
  id: string;
  icon: ReactNode;
  label: string;
  where?: string;
  shortcut?: string;
  perform: () => void;
}

/** The palette's commands. */
export function PaletteActions({ search, run }: PaletteActionsProps) {
  const activeTab = useSessionsStore(selectActiveTab);
  const themeId = useThemeStore((s) => s.themeId);
  const layout = useEditorLayoutStore((s) => s.layout);

  const actions: PaletteAction[] = [
    {
      id: "new-connection",
      icon: <Plus />,
      label: "New connection",
      shortcut: "Ctrl N",
      perform: () => useUiStore.getState().setConnectionDialog({ mode: "new" }),
    },
  ];
  if (activeTab) {
    const tab = activeTab;
    actions.push({
      id: "new-console",
      icon: <SquareTerminal />,
      label: `New console on ${tab.database}`,
      where: tab.connection.name,
      perform: () =>
        useSessionsStore
          .getState()
          .openConsole(tab.connection, tab.database, tab.kind === TabKind.Collection ? tab.collection : null),
    });
    actions.push({
      id: "open-to-side",
      icon: <PanelRight />,
      label: "Open the tab to the side",
      shortcut: "Ctrl \\",
      perform: () => {
        if (!useEditorLayoutStore.getState().openToSide(tab.id)) {
          toast("All four panes are in use", { description: "Close a pane, or drop the tab onto one to replace it." });
        }
      },
    });
    SPLIT_LAYOUT_ORDER.forEach((kind, i) => {
      if (kind === layout) return;
      actions.push({
        id: `layout-${kind}`,
        icon: <LayoutIcon layout={kind} />,
        label: kind === "single" ? "Editor: single pane" : `Split the editor: ${SPLIT_LAYOUTS[kind].label}`,
        shortcut: `Ctrl Alt ${i + 1}`,
        perform: () => useEditorLayoutStore.getState().setLayout(kind),
      });
    });
  }
  const other = themeId === "dark" ? "light" : "dark";
  actions.push(
    {
      id: "theme",
      icon: other === "light" ? <Sun /> : <Moon />,
      label: `Switch to ${other} theme`,
      perform: () => useThemeStore.getState().setTheme(other),
    },
    {
      id: "settings",
      icon: <Settings />,
      label: "Settings",
      shortcut: "Ctrl ,",
      perform: () => useUiStore.getState().setSettingsSection("general"),
    },
    {
      id: "mcp-settings",
      icon: <Server />,
      label: "MCP server settings",
      perform: () => useUiStore.getState().setSettingsSection("mcp"),
    },
    {
      id: "transfer",
      icon: <ArrowLeftRight />,
      label: "Import / export connections",
      perform: () => useUiStore.getState().openTransfer("export"),
    },
    {
      id: "delete-connections",
      icon: <Trash2 />,
      label: "Delete connections",
      perform: () => useUiStore.getState().openDeleteConnections(),
    },
    {
      id: "new-relation",
      icon: <Link2 />,
      label: activeTab?.kind === TabKind.Collection ? `Relate ${activeTab.collection} to another collection` : "New relation",
      perform: newRelationFromContext,
    },
    {
      id: "relations",
      icon: <Link2 />,
      label: "Show relations",
      perform: () => useUiStore.getState().setSidePanel("relations"),
    },
    {
      id: "toggle-explorer",
      icon: <Database />,
      label: "Toggle Explorer",
      shortcut: "Ctrl B",
      perform: () => useUiStore.getState().toggleSidePanel("explorer"),
    },
    {
      id: "assistant",
      icon: <Sparkles />,
      label: "Ask the Assistant",
      shortcut: "Ctrl L",
      perform: () => useAssistantStore.getState().openPanel(),
    },
    {
      id: "assistant-settings",
      icon: <SlidersHorizontal />,
      label: "Assistant settings",
      perform: () => useAssistantStore.getState().openPanel("setup"),
    },
    {
      id: "open-logs",
      icon: <FolderOpen />,
      label: "Open logs folder",
      perform: () =>
        api.openLogDir().catch((e) =>
          toast.error("Couldn't open the logs folder", { description: String(e) }),
        ),
    },
  );

  return (
    <CommandGroup heading="Actions">
      {actions.map((action) => (
        <CommandItem
          key={action.id}
          value={`action:${action.id}`}
          keywords={[action.label]}
          onSelect={() => run(action.perform)}
        >
          {action.icon}
          <span className="min-w-0 truncate">
            <HighlightMatch text={action.label} query={search} />
          </span>
          {action.where && <span className="shrink-0 text-sm text-fg-3">{action.where}</span>}
          {action.shortcut && <CommandShortcut>{action.shortcut}</CommandShortcut>}
        </CommandItem>
      ))}
    </CommandGroup>
  );
}
