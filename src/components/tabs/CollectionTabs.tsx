import { useCallback, useEffect, useRef, useState } from "react";
import type { MouseEvent } from "react";
import { Loader2, SquareTerminal, X } from "lucide-react";
import { useSessionsStore } from "../../store/sessionsStore";
import type { Tab } from "../../store/sessionsStore";
import { useScriptsStore } from "../../store/scriptsStore";
import { ContextMenu } from "../ui/ContextMenu";

interface TabButtonProps {
  tab: Tab;
  active: boolean;
  onContextMenu: (e: MouseEvent, tab: Tab) => void;
}

/** What a tab is called: its namespace, or for a console its database. */
function tabName(tab: Tab): string {
  if (tab.kind === "collection") return `${tab.database}.${tab.collection}`;
  return `${tab.database} console${tab.number > 1 ? ` ${tab.number}` : ""}`;
}

function TabButton({ tab, active, onContextMenu }: TabButtonProps) {
  const activateTab = useSessionsStore((s) => s.activateTab);
  const closeTab = useSessionsStore((s) => s.closeTab);
  const ref = useRef<HTMLDivElement>(null);
  const fileName = useScriptsStore((s) =>
    tab.kind === "console" ? s.files[tab.id]?.name : undefined,
  );

  // A tab opened from the sidebar may land past the strip's visible edge.
  useEffect(() => {
    if (active) ref.current?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }, [active]);

  return (
    <div
      ref={ref}
      role="tab"
      aria-selected={active}
      tabIndex={0}
      title={`${tabName(tab)}${fileName ? ` · ${fileName}` : ""}\n${tab.connection.name} · ${tab.connection.summary}`}
      className={`group flex max-w-[320px] shrink-0 cursor-pointer items-center gap-1.5 border-r border-t-2 border-r-border-subtle py-1.5 pl-3 pr-1.5 text-xs ${
        active
          ? "border-t-accent bg-editor text-text-default"
          : "border-t-transparent text-text-muted hover:bg-panel-hover hover:text-text-default"
      }`}
      onClick={() => activateTab(tab.id)}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          activateTab(tab.id);
        }
      }}
      // Middle-click closes, as in a browser. Swallow the mousedown too, or
      // it starts autoscroll on Linux/Windows.
      onMouseDown={(e) => {
        if (e.button === 1) e.preventDefault();
      }}
      onAuxClick={(e) => {
        if (e.button === 1) closeTab(tab.id);
      }}
      onContextMenu={(e) => onContextMenu(e, tab)}
    >
      {tab.kind === "collection" && tab.loading && (
        <Loader2 size={11} className="shrink-0 animate-spin text-text-faint" />
      )}
      {tab.kind === "console" && (
        <SquareTerminal size={12} className="shrink-0 text-text-muted" aria-hidden />
      )}
      <span className="truncate">
        {tab.kind === "collection" ? (
          <>
            <span className="text-text-faint">{tab.database}.</span>
            {tab.collection}
          </>
        ) : (
          // a console shows the script it's editing, once it has a file
          (fileName ?? tabName(tab))
        )}
        {/* Names the connection too, like mongosh's db@host, so tabs stay
            unambiguous when two servers hold the same namespace. */}
        <span className="ml-1 text-text-faint">@ {tab.connection.name}</span>
      </span>
      <button
        type="button"
        aria-label={`Close ${tabName(tab)} on ${tab.connection.name}`}
        title="Close (middle-click)"
        className={`shrink-0 rounded p-0.5 text-text-faint hover:bg-panel-alt hover:text-text-default focus:opacity-100 ${
          active ? "opacity-100" : "opacity-0 group-hover:opacity-100"
        }`}
        onClick={(e) => {
          e.stopPropagation();
          closeTab(tab.id);
        }}
      >
        <X size={12} />
      </button>
    </div>
  );
}

/** Open collections and consoles, across every database of every connection. */
export function CollectionTabs() {
  const tabs = useSessionsStore((s) => s.tabs);
  const activeTabId = useSessionsStore((s) => s.activeTabId);
  const closeTab = useSessionsStore((s) => s.closeTab);
  const closeOtherTabs = useSessionsStore((s) => s.closeOtherTabs);
  const closeAllTabs = useSessionsStore((s) => s.closeAllTabs);
  const [menu, setMenu] = useState<{ tabId: string; x: number; y: number } | null>(null);
  const closeMenu = useCallback(() => setMenu(null), []);

  function openMenu(e: MouseEvent, tab: Tab) {
    e.preventDefault();
    setMenu({ tabId: tab.id, x: e.clientX, y: e.clientY });
  }

  if (tabs.length === 0) return null;

  return (
    <div
      role="tablist"
      aria-label="Open tabs"
      // No visible scrollbar - it would squeeze the labels - so a plain mouse
      // wheel scrolls the strip sideways instead, like editor tab bars.
      className="flex shrink-0 overflow-x-auto border-b border-border-subtle bg-sidebar scrollbar-none"
      onWheel={(e) => {
        if (e.deltaX === 0) e.currentTarget.scrollLeft += e.deltaY;
      }}
    >
      {tabs.map((tab) => (
        <TabButton
          key={tab.id}
          tab={tab}
          active={tab.id === activeTabId}
          onContextMenu={openMenu}
        />
      ))}
      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          onClose={closeMenu}
          items={[
            { label: "Close tab", onSelect: () => closeTab(menu.tabId) },
            {
              label: "Close other tabs",
              onSelect: () => closeOtherTabs(menu.tabId),
              disabled: tabs.length < 2,
            },
            { label: "Close all tabs", onSelect: closeAllTabs },
          ]}
        />
      )}
    </div>
  );
}
