import { Copy, Funnel, ListChecks, Trash2, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Kbd } from "@/components/ui/kbd";
import type { CollectionTab } from "../../store/sessionsStore";
import { ResultViewToggle } from "../json/ResultViewToggle";
import { resultViewKey, useResultView } from "../../store/uiStore";
import { ActionMenuButton, type MenuEntry } from "../common/ActionMenu";
import { askDeleteDocuments, askDeleteMatching, canDeleteFrom, copyDocuments } from "./deleteDocuments";
import { clearPicks, selectAllOf, usePickedIndexes } from "./useDocumentSelection";
import { LegacyUuidHint } from "./LegacyUuidHint";
import { ResultsPager } from "./ResultsPager";
import { RelatedMenu } from "./related/RelatedMenu";

/** A find with no filter matches the whole collection, whose count the stats have. */
function knownTotal(tab: CollectionTab): number | null {
  if (tab.resultsMode !== "find" || !tab.stats) return null;
  // the filter that produced the results, not the field's text since
  const filter = (tab.resultsFilter ?? tab.filterText).replace(/\s/g, "");
  return filter === "" || filter === "{}" ? tab.stats.documentCount : null;
}

/**
 * The 34px line over a tab's results: "<n> documents · showing a–b · <ms> ms",
 * then the Related menu, the result view switch and the pager.
 */
export function ResultsHeader({ tab }: { tab: CollectionTab }) {
  const { results, loading, queryMs, skip } = tab;
  const returned = results?.documents.length ?? 0;
  const total = knownTotal(tab);
  const count = total ?? returned;
  const from = tab.resultsMode === "find" ? skip : 0;
  const documents = results?.documents ?? NO_DOCUMENTS;
  const view = useResultView(resultViewKey(tab.id, "documents"));
  const grid = view === "grid";
  const picked = usePickedIndexes(tab.id, documents);
  const pickedDocs = grid ? picked.map((i) => documents[i]) : [];
  const deletable = canDeleteFrom(tab);
  const filterless = (tab.resultsFilter ?? tab.filterText).replace(/\s/g, "");

  const moreEntries = (): MenuEntry[] => [
    { heading: deletable ? `${tab.database}.${tab.collection}` : `${tab.database}.${tab.collection} · read-only results` },
    ...(grid ? [{ label: "Select all on this page", icon: ListChecks, shortcut: "Ctrl A", onSelect: () => selectAllOf(tab.id, documents) }] : []),
    { separator: true },
    {
      label: pickedDocs.length > 1 ? `Delete ${pickedDocs.length} selected documents…` : "Delete the selected document…",
      icon: Trash2,
      danger: deletable,
      disabled: !deletable || !grid || pickedDocs.length === 0,
      shortcut: "Del",
      onSelect: () => askDeleteDocuments(tab.id, pickedDocs),
    },
    {
      label: filterless === "" || filterless === "{}" ? "Delete every document in the collection…" : "Delete every document matching the filter…",
      icon: Funnel,
      danger: deletable,
      disabled: !deletable,
      onSelect: () => askDeleteMatching(tab.id),
    },
  ];

  if (results && pickedDocs.length > 1) {
    return (
      <div className="flex h-[34px] shrink-0 items-center gap-2 border-b border-line pr-2.5 pl-1.5 text-sm text-fg-2">
        <Button variant="ghost" size="icon-sm" title="Clear the selection (Esc)" aria-label="Clear the selection" onClick={() => clearPicks(tab.id)}>
          <X />
        </Button>
        <span className="text-fg">
          <b className="font-semibold tabular-nums">{pickedDocs.length}</b> of {returned.toLocaleString("en")} selected
        </span>
        <Button variant="ghost" size="sm" onClick={() => void copyDocuments(pickedDocs)}>
          <Copy />
          Copy as JSON
        </Button>
        {deletable && (
          <Button variant="ghost" size="sm" className="text-danger hover:bg-danger/12 hover:text-danger" onClick={() => askDeleteDocuments(tab.id, pickedDocs)}>
            <Trash2 />
            Delete {pickedDocs.length}…<Kbd className="ml-0.5">Del</Kbd>
          </Button>
        )}
        <span className="flex-1" />
        <RelatedMenu tab={tab} />
        <ResultViewToggle viewKey={resultViewKey(tab.id, "documents")} />
        <ResultsPager tab={tab} total={total} />
        <ActionMenuButton entries={moreEntries} label="More result actions" align="end" />
      </div>
    );
  }

  return (
    <div className="flex h-[34px] shrink-0 items-center gap-3 border-b border-line pr-2.5 pl-3 text-sm text-fg-2">
      {results ? (
        <span>
          <span className="font-medium text-fg tabular-nums">{count.toLocaleString("en")}</span>{" "}
          {count === 1 ? "document" : "documents"}
        </span>
      ) : (
        <span>{loading ? "Running query…" : "Run a query to see documents"}</span>
      )}
      {results && returned > 0 && (
        <span className="tabular-nums max-sm:hidden">
          showing {(from + 1).toLocaleString("en")}–{(from + returned).toLocaleString("en")}
        </span>
      )}
      {results && loading && <span className="max-sm:hidden">running…</span>}
      {results && !loading && queryMs !== null && (
        <span className="tabular-nums max-sm:hidden">{queryMs.toLocaleString("en")} ms</span>
      )}
      <span className="flex-1" />
      <LegacyUuidHint tab={tab} />
      <RelatedMenu tab={tab} />
      <ResultViewToggle viewKey={resultViewKey(tab.id, "documents")} />
      <ResultsPager tab={tab} total={total} />
      {results && <ActionMenuButton entries={moreEntries} label="More result actions" align="end" />}
    </div>
  );
}

const NO_DOCUMENTS: unknown[] = [];
