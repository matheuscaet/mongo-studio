import { useContext } from "react";
import type { ReactNode } from "react";
import { Kbd } from "@/components/ui/kbd";
import { Copy, Trash2, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { countLabel, documentIdText, documentKey, documentLabel } from "../../lib/bsonFormat";
import { isPlainObject } from "../../lib/bsonValue";
import { TabKind, useSessionsStore } from "../../store/sessionsStore";
import { askDeleteDocuments, canDeleteFrom, copyDocuments } from "./deleteDocuments";
import { DocumentActions } from "../grid/DocumentActions";
import { JsonTree } from "../json/JsonTree";
import { ValueEditContext } from "../json/ValueEditContext";

interface DocumentInspectorProps {
  /** The selected document; undefined when nothing is selected. */
  doc: unknown;
  collectionName: string;
  tabId: string;
  /** Under the fields: the document's related documents. */
  related?: ReactNode;
  /** Several documents picked in the grid: they're listed instead of one document's fields. */
  picked?: unknown[];
  /** Their row numbers - 1. */
  pickedIndexes?: number[];
  onClearPicks?: () => void;
}

function InspectorFooter() {
  const editable = useContext(ValueEditContext) !== null;
  return (
    <div className="flex shrink-0 flex-wrap gap-x-3 gap-y-1 border-t border-line-soft px-3 py-[7px] text-xs text-fg-3">
      {editable ? (
        <>
          <span>Double-click a value or a name to edit</span>
          <span>Right-click for more</span>
          <span>
            <Kbd>Enter</Kbd> saves
          </span>
          <span>
            <Kbd>Esc</Kbd> cancels
          </span>
        </>
      ) : (
        <span>Read-only results: aggregate output can't be edited or deleted</span>
      )}
    </div>
  );
}

/**
 * The selected document as a typed key/value tree beside the grid, with its
 * copy and edit-in-console actions, and its related documents under it.
 */
export function DocumentInspector({
  doc,
  collectionName,
  tabId,
  related,
  picked,
  pickedIndexes,
  onClearPicks,
}: DocumentInspectorProps) {
  if (picked && picked.length > 1) {
    return (
      <PickedDocuments documents={picked} indexes={pickedIndexes ?? []} tabId={tabId} onClear={onClearPicks} />
    );
  }
  if (doc === undefined) {
    return (
      <section aria-label="Document" className="flex h-full flex-col bg-editor">
        <p className="m-auto p-6 text-center text-fg-3">
          Select a document to see all of its fields.
        </p>
      </section>
    );
  }

  const id = documentIdText(doc);
  return (
    <section aria-label="Document" className="flex h-full min-h-0 flex-col bg-editor">
      <header className="flex h-9 shrink-0 items-center gap-1.5 border-b border-line-soft pr-1.5 pl-3">
        <h2 className="text-base font-semibold text-fg">Document</h2>
        <span className="min-w-0 flex-1 truncate font-mono text-xs text-fg-3" title={id ?? undefined}>
          {id}
        </span>
        <DocumentActions doc={doc} collectionName={collectionName} tabId={tabId} />
      </header>
      <div className="min-h-0 flex-1 overflow-auto pt-1 pb-4">
        <JsonTree
          // a new tree per document, so expanded nodes don't carry over
          key={documentKey(doc) ?? undefined}
          value={doc}
          defaultOpenDepth={1}
          aria-label="Document fields"
        />
        {related}
      </div>
      <InspectorFooter />
    </section>
  );
}

/** The documents picked in the grid, listed, with copy and delete for all of them. */
function PickedDocuments({
  documents,
  indexes,
  tabId,
  onClear,
}: {
  documents: unknown[];
  indexes: number[];
  tabId: string;
  onClear?: () => void;
}) {
  const deletable = useSessionsStore((s) => {
    const tab = s.tabs.find((t) => t.id === tabId);
    return tab?.kind === TabKind.Collection && canDeleteFrom(tab);
  });
  const n = documents.length;
  return (
    <section aria-label="Picked documents" className="flex h-full min-h-0 flex-col bg-editor">
      <header className="flex h-9 shrink-0 items-center gap-1.5 border-b border-line-soft pr-1.5 pl-3">
        <h2 className="text-base font-semibold text-fg">{n} documents</h2>
        <span className="min-w-0 flex-1 truncate text-xs text-fg-3">picked on this page</span>
        <Button variant="ghost" size="icon" title="Copy as JSON" aria-label={`Copy the ${n} documents as JSON`} className="[&_svg]:size-3.5" onClick={() => void copyDocuments(documents)}>
          <Copy />
        </Button>
        {deletable && (
          <Button variant="ghost" size="icon" title={`Delete ${n} documents… (Del)`} aria-label={`Delete ${n} documents`} className="hover:text-danger [&_svg]:size-3.5" onClick={() => askDeleteDocuments(tabId, documents)}>
            <Trash2 />
          </Button>
        )}
        {onClear && (
          <Button variant="ghost" size="icon" title="Clear the selection (Esc)" aria-label="Clear the selection" className="[&_svg]:size-3.5" onClick={onClear}>
            <X />
          </Button>
        )}
      </header>
      <ul className="m-0 min-h-0 flex-1 list-none overflow-auto p-0 pt-1 font-data">
        {documents.map((d, i) => (
          <li key={i} className="flex min-h-6 items-center gap-2 py-0.5 pr-2.5 pl-[22px] whitespace-nowrap">
            <span className="text-fg-3">{(indexes[i] ?? i) + 1}:</span>
            <span className="min-w-0 truncate">
              <span className="text-json-bson">{documentIdText(d)}</span>
              {documentLabel(d) && <span className="text-fg-3"> · {documentLabel(d)}</span>}
            </span>
            <span className="ml-auto pl-2 font-sans text-[10px] text-fg-3">
              {countLabel(isPlainObject(d) ? Object.keys(d).length : 0, "field")}
            </span>
          </li>
        ))}
      </ul>
      <div className="flex shrink-0 flex-wrap gap-x-3 gap-y-1 border-t border-line-soft px-3 py-[7px] text-xs text-fg-3">
        <span>
          <Kbd>Ctrl</Kbd>-click adds or removes
        </span>
        <span>
          <Kbd>Shift</Kbd>-click a range
        </span>
        {deletable && (
          <span>
            <Kbd>Del</Kbd> deletes
          </span>
        )}
        <span>
          <Kbd>Esc</Kbd> clears
        </span>
      </div>
    </section>
  );
}
