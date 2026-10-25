import type { CollectionTab } from "../../store/sessionsStore";
import { ResultViewToggle } from "../json/ResultViewToggle";
import { resultViewKey } from "../../store/uiStore";
import { LegacyUuidHint } from "./LegacyUuidHint";
import { ResultsPager } from "./ResultsPager";
import { RelatedMenu } from "./related/RelatedMenu";

/** A find with no filter matches the whole collection, whose count the stats have. */
function knownTotal(tab: CollectionTab): number | null {
  if (tab.resultsMode !== "find" || !tab.stats) return null;
  const filter = tab.filterText.replace(/\s/g, "");
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
    </div>
  );
}
