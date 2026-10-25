import type { CSSProperties } from "react";
import { CollectionTabView } from "../collection/CollectionTabView";
import { ScriptConsole } from "../console/ScriptConsole";
import { RelationEditor } from "../relations/RelationEditor";
import { DormantTab } from "./DormantTab";
import { CollectionTabs } from "../tabs/CollectionTabs";
import { TabDragGhost } from "../tabs/TabDragGhost";
import { useTabDragStore } from "../tabs/useTabDrag";
import { LegacyUuidContext } from "../json/LegacyUuidContext";
import { SPLIT_LAYOUTS } from "../../lib/editorLayout";
import { useConnectionsStore } from "../../store/connectionsStore";
import { useEditorLayoutStore } from "../../store/editorLayoutStore";
import { selectActiveTab, type Tab, TabKind, useSessionsStore } from "../../store/sessionsStore";
import { cn } from "@/lib/utils";
import { EditorPaneHeader } from "./EditorPaneHeader";
import { EmptyPane } from "./EmptyPane";
import { PaneContext } from "./paneContext";
import { ShortcutWatermark } from "./ShortcutWatermark";
import { WelcomeScreen } from "./WelcomeScreen";
import { useConnectedCount } from "./useActiveConnection";

/** The pane header's height, in px. */
const PANE_HEADER = 30;

/**
 * The editor: the tab strip and, under it, one view or up to four panes.
 * Everything sits in one CSS grid - a header row and a body row per pane
 * row - and each tab's view is placed in its pane's body or kept hidden, so
 * changing layout or moving a tab to another pane never remounts it: its
 * scroll position and expanded nodes survive.
 */
export function EditorArea() {
  const connected = useConnectedCount();
  const tabs = useSessionsStore((s) => s.tabs);
  const activeTab = useSessionsStore(selectActiveTab);
  const layout = useEditorLayoutStore((s) => s.layout);
  const panes = useEditorLayoutStore((s) => s.panes);
  const focused = useEditorLayoutStore((s) => s.focused);
  const maximized = useEditorLayoutStore((s) => s.maximized);
  const focusPane = useEditorLayoutStore((s) => s.focusPane);
  const dragging = useTabDragStore((s) => s.tabId !== null);
  const dropTarget = useTabDragStore((s) => s.target);

  if (connected === 0 && !activeTab) return <WelcomeScreen />;

  const split = layout !== "single";
  // The panes on screen: all of them, or the maximized one alone.
  const shown = !split ? [] : maximized !== null ? [maximized] : panes.map((_, i) => i);
  const info = SPLIT_LAYOUTS[maximized !== null ? "single" : layout];
  const cols = split ? info.cols : 1;
  const rows = split ? info.rows : 1;
  const compact = split && maximized === null && (layout === "cols3" || layout === "grid");

  /** Grid placement of a pane's header and body, by its position on screen. */
  const cell = (slot: number, part: "header" | "body" | "both"): CSSProperties => {
    const col = (slot % cols) + 1;
    if (!split) return { gridRow: 1, gridColumn: 1 };
    const row = Math.floor(slot / cols) * 2 + 1;
    const gridRow = part === "header" ? row : part === "body" ? row + 1 : `${row} / span 2`;
    return { gridRow, gridColumn: col };
  };
  const slotOf = (tabId: string) => (split ? shown.findIndex((p) => panes[p] === tabId) : tabId === activeTab?.id ? 0 : -1);

  return (
    <>
      {/* Tabs reopened from the last session show before anything connects. */}
      {(connected > 0 || tabs.length > 0) && <CollectionTabs />}
      <div
        className={cn("relative grid min-h-0 flex-1", split ? "gap-px bg-line" : "bg-editor")}
        style={{
          gridTemplateColumns: `repeat(${cols}, minmax(0, 1fr))`,
          gridTemplateRows: split ? `repeat(${rows}, ${PANE_HEADER}px minmax(0, 1fr))` : "minmax(0, 1fr)",
        }}
      >
        {shown.map((p, slot) => {
          const tab = tabs.find((t) => t.id === panes[p]) ?? null;
          return (
            <EditorPaneHeader
              key={`head-${p}`}
              pane={p}
              tab={tab}
              focused={p === focused}
              maximized={maximized !== null}
              style={cell(slot, "header")}
            />
          );
        })}
        {shown.map((p, slot) =>
          panes[p] === null ? <EmptyPane key={`empty-${p}`} pane={p} focused={p === focused} style={cell(slot, "body")} /> : null,
        )}

        {/* Every collection tab stays mounted; the ones no pane shows are
            hidden in place, keeping their scroll and expanded nodes. */}
        {tabs.map((tab) => {
          const slot = slotOf(tab.id);
          const visible = slot >= 0;
          const pane = split && visible ? shown[slot] : -1;
          const isFocused = split ? pane === focused : visible;
          if (tab.kind === TabKind.Console && !visible) return null; // a console tab is only a console
          return (
            <div
              key={tab.id}
              style={visible ? cell(slot, "body") : undefined}
              className={cn("min-h-0 min-w-0", visible ? "relative" : "invisible absolute inset-0")}
              // A click or Tab key into a pane focuses it, so shortcuts and
              // the next tab clicked go there.
              onPointerDownCapture={split && visible ? () => focusPane(pane) : undefined}
              onFocusCapture={split && visible ? () => focusPane(pane) : undefined}
            >
              <PaneContext.Provider value={{ inPane: split, compact }}>
                {tab.dormant ? visible && <DormantTab tab={tab} /> : <TabBody tab={tab} active={isFocused} shown={visible} />}
              </PaneContext.Provider>
            </div>
          );
        })}

        {!split && !activeTab && <ShortcutWatermark />}

        {/* While a tab is dragged: each pane is a drop target, and a single
            view offers its right half to split. */}
        {dragging &&
          shown.map((p, slot) => (
            <div
              key={`drop-${p}`}
              data-drop-pane={p}
              style={cell(slot, "both")}
              className={cn(
                "relative z-10",
                dropTarget?.kind === "pane" && dropTarget.pane === p && "bg-accent/12 ring-2 ring-accent ring-inset",
              )}
            />
          ))}
        {dragging && !split && (
          <div
            data-drop-side=""
            className={cn(
              "absolute inset-y-0 right-0 z-10 grid w-1/2 place-items-center border-l border-dashed border-accent",
              dropTarget?.kind === "side" ? "bg-accent/12" : "bg-accent/5",
            )}
          >
            <span className="rounded-md bg-panel px-2.5 py-1.5 text-sm font-medium text-accent-text shadow-overlay">
              Drop to open side by side
            </span>
          </div>
        )}
      </div>
      <TabDragGhost />
    </>
  );
}

/** What a live tab shows, by its kind, reading legacy UUIDs as its connection does. */
function TabBody({ tab, active, shown }: { tab: Tab; active: boolean; shown: boolean }) {
  const legacyUuid = useConnectionsStore(
    (s) => s.profiles.find((p) => p.id === tab.connection.id)?.legacyUuid ?? "raw",
  );
  return (
    <LegacyUuidContext.Provider value={legacyUuid}>
      <TabKindBody tab={tab} active={active} shown={shown} />
    </LegacyUuidContext.Provider>
  );
}

function TabKindBody({ tab, active, shown }: { tab: Tab; active: boolean; shown: boolean }) {
  switch (tab.kind) {
    case TabKind.Collection:
      return <CollectionTabView tab={tab} active={active} shown={shown} />;
    case TabKind.Console:
      return <ScriptConsole tab={tab} active={active} />;
    case TabKind.Relation:
      return <RelationEditor tab={tab} active={active} />;
    default: {
      // a new kind of tab has to say what it shows
      const unhandled: never = tab;
      return unhandled;
    }
  }
}
