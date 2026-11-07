import { useCallback, useMemo } from "react";
import { create } from "zustand";
import { documentKey } from "../../lib/bsonFormat";

interface Selection {
  /** The anchor: the row the inspector shows, and a Shift-click range starts from. */
  index: number;
  /** The anchor's `documentKey`, to find it again in new results. */
  key: string | null;
  /** The result page it was selected in. */
  documents: unknown[];
  /** Rows picked besides the anchor, by `documentKey`. */
  extra: string[];
}

// Per tab, outliving the results pane: switching to Indexes and back keeps it.
const useSelectionStore = create<{ byTab: Record<string, Selection> }>(() => ({ byTab: {} }));

function setSelection(tabId: string, selection: Selection) {
  useSelectionStore.setState((s) => ({ byTab: { ...s.byTab, [tabId]: selection } }));
}

function resolve(selection: Selection | undefined, documents: unknown[]): number | null {
  if (documents.length === 0) return null;
  if (!selection) return 0;
  if (selection.documents === documents) {
    return selection.index < documents.length ? selection.index : 0;
  }
  // New results (an in-place edit replaces the page too): follow the same
  // document if it is still there, otherwise start over at the first row.
  if (selection.key === null) return 0;
  const found = documents.findIndex((d) => documentKey(d) === selection.key);
  return found === -1 ? 0 : found;
}

/** Every picked row of the page, in page order: the anchor and the extra rows still in it. */
function pickedOf(selection: Selection | undefined, documents: unknown[]): number[] {
  const anchor = resolve(selection, documents);
  if (anchor === null) return [];
  const extra = new Set(selection?.extra ?? []);
  if (extra.size === 0) return [anchor];
  const out: number[] = [];
  documents.forEach((d, i) => {
    if (i === anchor) out.push(i);
    else {
      const key = documentKey(d);
      if (key !== null && extra.has(key)) out.push(i);
    }
  });
  return out;
}

/** The document selected in a tab's grid, read outside React (the Assistant's context). */
export function selectedDocumentOf(tabId: string, documents: unknown[]): unknown {
  const index = resolve(useSelectionStore.getState().byTab[tabId], documents);
  return index === null ? undefined : documents[index];
}

/** The rows picked in a tab's grid, read outside React. */
export function pickedIndexesOf(tabId: string, documents: unknown[]): number[] {
  return pickedOf(useSelectionStore.getState().byTab[tabId], documents);
}

/** Forgets a tab's picked rows, keeping its anchor (e.g. after they were deleted). */
export function clearPicks(tabId: string) {
  const selection = useSelectionStore.getState().byTab[tabId];
  if (selection && selection.extra.length > 0) setSelection(tabId, { ...selection, extra: [] });
}

/** Selects one row alone, read outside React. */
export function selectRow(tabId: string, documents: unknown[], i: number) {
  setSelection(tabId, { index: i, key: documentKey(documents[i]), documents, extra: [] });
}

/** A Ctrl/Cmd-click (toggle) or Shift-click (range from the anchor) on row `i`. */
export function pickRow(tabId: string, documents: unknown[], i: number, how: "toggle" | "range") {
  const current = useSelectionStore.getState().byTab[tabId];
  const anchor = resolve(current, documents);
  if (anchor === null) return;
  const base = { index: anchor, key: documentKey(documents[anchor]), documents };
  const keyAt = (n: number) => documentKey(documents[n]);
  if (how === "range") {
    const [lo, hi] = anchor < i ? [anchor, i] : [i, anchor];
    const extra: string[] = [];
    for (let n = lo; n <= hi; n++) {
      const key = keyAt(n);
      if (n !== anchor && key !== null) extra.push(key);
    }
    setSelection(tabId, { ...base, extra });
    return;
  }
  const others = pickedOf(current, documents).filter((n) => n !== anchor);
  const extra = new Set(others.map(keyAt).filter((k): k is string => k !== null));
  if (i === anchor) {
    // un-picking the anchor hands it to the first row still picked
    const next = others[0];
    if (next === undefined) return;
    const key = keyAt(next);
    if (key !== null) extra.delete(key);
    setSelection(tabId, { index: next, key, documents, extra: [...extra] });
    return;
  }
  const key = keyAt(i);
  if (key === null) return;
  if (extra.has(key)) extra.delete(key);
  else extra.add(key);
  setSelection(tabId, { ...base, extra: [...extra] });
}

/** Picks every row of the page, keeping the anchor. */
export function selectAllOf(tabId: string, documents: unknown[]) {
  const anchor = resolve(useSelectionStore.getState().byTab[tabId], documents);
  if (anchor === null) return;
  const extra = documents
    .map((d, n) => (n === anchor ? null : documentKey(d)))
    .filter((k): k is string => k !== null);
  setSelection(tabId, { index: anchor, key: documentKey(documents[anchor]), documents, extra });
}

/** The rows picked in a tab's grid, for views beside it (the results header). */
export function usePickedIndexes(tabId: string, documents: unknown[]): number[] {
  const selection = useSelectionStore((s) => s.byTab[tabId]);
  return useMemo(() => pickedOf(selection, documents), [selection, documents]);
}

export interface DocumentSelection {
  /** The anchor row: the inspector's document. Null on an empty page. */
  index: number | null;
  /** Every picked row, anchor included, in page order. */
  picked: number[];
  /** Selects one row alone. */
  select: (index: number) => void;
  /** Ctrl/Cmd-click toggles a row; Shift-click picks the range from the anchor. */
  pick: (index: number, how: "toggle" | "range") => void;
  selectAll: () => void;
  /** Back to the anchor alone. */
  clear: () => void;
}

/**
 * The grid's selection for a tab: an anchor row (the first row until one
 * is picked) and, with Ctrl/Cmd- or Shift-clicks, more rows besides it.
 * Rows are remembered by `documentKey`, so they survive an in-place edit
 * replacing the page.
 */
export function useDocumentSelection(tabId: string, documents: unknown[]): DocumentSelection {
  const selection = useSelectionStore((s) => s.byTab[tabId]);
  const index = useMemo(() => resolve(selection, documents), [selection, documents]);
  const picked = useMemo(() => pickedOf(selection, documents), [selection, documents]);

  const select = useCallback((i: number) => selectRow(tabId, documents, i), [tabId, documents]);

  const pick = useCallback(
    (i: number, how: "toggle" | "range") => pickRow(tabId, documents, i, how),
    [tabId, documents],
  );

  const selectAll = useCallback(() => selectAllOf(tabId, documents), [tabId, documents]);

  const clear = useCallback(() => clearPicks(tabId), [tabId]);

  return { index, picked, select, pick, selectAll, clear };
}
