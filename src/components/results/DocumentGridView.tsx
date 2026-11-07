import { useCallback, useEffect, useMemo, useState } from "react";
import { Copy, ListChecks, SquareTerminal, Trash2 } from "lucide-react";
import { cn } from "@/lib/utils";
import type { RelationEnd } from "../../lib/relations";
import { useRelatedPages } from "../../store/relatedStore";
import type { CollectionTab } from "../../store/sessionsStore";
import { DEFAULT_INSPECTOR_WIDTH, useUiStore } from "../../store/uiStore";
import { DocumentTable } from "../grid/DocumentTable";
import { usePane } from "../layout/paneContext";
import { DocumentInspector } from "./DocumentInspector";
import { InlineDocument } from "./InlineDocument";
import { InspectorResizer } from "./InspectorResizer";
import { RelatedChips } from "./related/RelatedChips";
import { RelatedSection } from "./related/RelatedSection";
import { useRelatedSource } from "./related/relatedActions";
import { useRelatedGrid } from "./related/useRelatedGrid";
import { useDocumentSelection } from "./useDocumentSelection";
import { askDeleteDocuments, canDeleteFrom, copyDocuments } from "./deleteDocuments";
import type { MenuEntry } from "../common/ActionMenu";
import { buildEditScript } from "../../lib/editScript";
import { documentLabel } from "../../lib/bsonFormat";
import { isPlainObject } from "../../lib/bsonValue";
import { useConsoleStore } from "../../store/consoleStore";
import { useSessionsStore } from "../../store/sessionsStore";
import { useElementWidth } from "./useElementWidth";
import { useMediaQuery } from "./useMediaQuery";

const NO_ENDS: RelationEnd[] = [];

/** The inspector's default width on narrower windows. */
const NARROW_INSPECTOR_WIDTH = 320;
/** Below this width the grid and the inspector stack: the grid would be too narrow beside it. */
const STACK_BELOW = 780;
/** A pane narrower than this has no room for the inspector even stacked: documents open under their rows. */
const INLINE_BELOW = 600;

interface DocumentGridViewProps {
  tab: CollectionTab;
  documents: unknown[];
}

/**
 * The grid with the selected document in an inspector beside it, or under it
 * on narrow windows. In a narrow pane of a split the inspector goes, and a
 * click opens the whole document under its row instead. Related columns
 * follow the document's, and the inspector lists the related documents.
 */
export function DocumentGridView({ tab, documents }: DocumentGridViewProps) {
  const selection = useDocumentSelection(tab.id, documents);
  const { index: selected, select, picked } = selection;
  const pane = usePane();
  // only a find's documents are the collection's, as stored: aggregate output has no related documents
  const find = tab.resultsMode === "find";
  const related = useRelatedPages(useRelatedSource(tab), find ? documents : null);
  const view = useRelatedGrid(tab.id, tab.collection, documents, find ? related.ends : NO_ENDS, related.pages);
  // With a Related column's "only these" filter on, the table shows part of
  // the page: its rows map back to the page's documents.
  const { shown, indexes, filtered } = view;
  const position = selected === null ? null : filtered ? indexes.indexOf(selected) : selected;
  const shownSelected = position === -1 ? null : position;
  const selectShown = useCallback((i: number) => select(indexes[i] ?? i), [select, indexes]);
  const { pick, selectAll, clear } = selection;
  const pickShown = useCallback((i: number, how: "toggle" | "range") => pick(indexes[i] ?? i, how), [pick, indexes]);
  const pickedShown = useMemo(() => {
    if (!filtered) return new Set(picked);
    const at = new Map(indexes.map((page, i) => [page, i]));
    return new Set(picked.map((p) => at.get(p)).filter((i): i is number => i !== undefined));
  }, [picked, filtered, indexes]);
  const pickedDocs = useMemo(() => picked.map((i) => documents[i]), [picked, documents]);
  const deletable = canDeleteFrom(tab);
  const deletePicked = useCallback(() => {
    if (deletable) askDeleteDocuments(tab.id, pickedDocs);
  }, [deletable, tab.id, pickedDocs]);
  const rowMenu = useCallback(
    (shown: number): MenuEntry[] => {
      const n = pickedDocs.length;
      const doc = documents[indexes[shown] ?? shown];
      const label = n > 1 ? `${n} documents` : (documentLabel(doc) ?? `document ${shown + 1}`);
      return [
        { heading: `${tab.database}.${tab.collection} · ${label}` },
        { label: n > 1 ? `Copy ${n} documents as JSON` : "Copy as JSON", icon: Copy, onSelect: () => void copyDocuments(pickedDocs) },
        ...(n === 1 && isPlainObject(doc)
          ? [{
              label: "Edit in console (updateOne)",
              icon: SquareTerminal,
              onSelect: () => {
                useConsoleStore.getState().setScript(tab.id, buildEditScript(doc, tab.collection));
                useSessionsStore.getState().activateTab(tab.id);
                useUiStore.getState().setMainTab(tab.id, "console");
              },
            }]
          : []),
        { label: "Select all on this page", icon: ListChecks, shortcut: "Ctrl A", onSelect: selectAll },
        { separator: true },
        deletable
          ? { label: n > 1 ? `Delete ${n} documents…` : "Delete document…", icon: Trash2, danger: true, shortcut: "Del", onSelect: deletePicked }
          : { label: "Delete document… · read-only results", icon: Trash2, disabled: true, onSelect: () => {} },
      ];
    },
    [pickedDocs, documents, indexes, tab.id, tab.database, tab.collection, selectAll, deletable, deletePicked],
  );
  const [expanded, setExpanded] = useState<number | null>(null);
  // A new result closes the open document: its row now holds another one.
  useEffect(() => setExpanded(null), [shown]);
  const table = {
    documents: shown,
    columnsFrom: filtered ? documents : undefined,
    rowNumbers: filtered ? indexes.map((i) => i + 1) : undefined,
    related: view.grid,
    selectedIndex: shownSelected,
    onSelect: selectShown,
    picked: pickedShown,
    onPick: pickShown,
    onSelectAll: selectAll,
    onClearPicks: clear,
    onDelete: deletable ? deletePicked : undefined,
    rowMenu,
  };
  const chips = <RelatedChips tabId={tab.id} ends={find ? related.ends : NO_ENDS} />;
  const storedWidth = useUiStore((s) => s.inspectorWidth);
  const [box, boxWidth] = useElementWidth<HTMLDivElement>();
  const stacked = useMediaQuery("(max-width: 959px)") || (boxWidth > 0 && boxWidth < STACK_BELOW);
  const narrow = useMediaQuery("(max-width: 1179px)");
  // Narrow windows start the inspector smaller; a width the user dragged to stays.
  const width =
    narrow && storedWidth === DEFAULT_INSPECTOR_WIDTH ? NARROW_INSPECTOR_WIDTH : storedWidth;
  const inline = pane.compact || (pane.inPane && boxWidth > 0 && boxWidth < INLINE_BELOW);

  if (inline) {
    return (
      <>
      {chips}
      <div ref={box} className="flex min-h-0 flex-1">
        <DocumentTable
          {...table}
          expandedIndex={expanded}
          onToggleExpand={(i) => setExpanded((open) => (open === i ? null : i))}
          renderExpanded={(i, viewport) => (
            <InlineDocument
              doc={shown[i]}
              index={indexes[i] ?? i}
              width={viewport}
              collectionName={tab.collection}
              tabId={tab.id}
              onClose={() => setExpanded(null)}
            />
          )}
          aria-label={`Documents in ${tab.database}.${tab.collection}`}
          className="flex-1"
        />
      </div>
      </>
    );
  }

  return (
    <>
    {chips}
    <div ref={box} className={cn("flex min-h-0 flex-1", stacked && "flex-col")}>
      <DocumentTable
        {...table}
        fadeRight={!stacked}
        aria-label={`Documents in ${tab.database}.${tab.collection}`}
        className="flex-1"
      />
      <aside
        className={cn(
          "relative flex shrink-0 flex-col",
          stacked ? "h-[45%] border-t border-line" : "border-l border-line",
        )}
        style={stacked ? undefined : { width }}
      >
        {!stacked && <InspectorResizer width={width} />}
        <DocumentInspector
          doc={selected === null ? undefined : documents[selected]}
          picked={pickedDocs.length > 1 ? pickedDocs : undefined}
          pickedIndexes={picked}
          onClearPicks={clear}
          collectionName={tab.collection}
          tabId={tab.id}
          related={
            selected !== null &&
            find && (
              <RelatedSection
                tab={tab}
                doc={documents[selected]}
                index={selected}
                ends={related.ends}
                pages={related.pages}
              />
            )
          }
        />
      </aside>
    </div>
    </>
  );
}
