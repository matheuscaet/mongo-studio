import { useState } from "react";
import { CircleCheck, LoaderCircle, Trash2, TriangleAlert } from "lucide-react";
import { Button } from "@/components/ui/button";
import { ConnectionChip } from "@/components/ui/ConnectionChip";
import { Input } from "@/components/ui/input";
import { Modal } from "@/components/ui/Modal";
import { documentLabel } from "../../lib/bsonFormat";
import { bsonLiteral } from "../../lib/bsonValue";
import { useConnectionColor } from "../../lib/connectionColor";
import { compactQueryText } from "../../lib/queryText";
import { TabKind, useSessionsStore } from "../../store/sessionsStore";
import { closeDeleteDialog, confirmDelete, useDeleteStore, type DeleteRequest } from "./deleteDocuments";

/** How many documents the dialog lists before "and n more". */
const LISTED = 5;

const plural = (n: number) => `${n.toLocaleString("en")} ${n === 1 ? "document" : "documents"}`;

function idText(id: unknown): string {
  return bsonLiteral(id) ?? JSON.stringify(id);
}

/** The statement the delete runs, as mongosh would read it. */
function statement(request: DeleteRequest, collection: string): string {
  if (request.kind === "filter") {
    return `db.${collection}.deleteMany(${compactQueryText(request.filterText) ?? request.filterText})`;
  }
  const ids = request.documents.map((d) => idText(d._id));
  if (ids.length === 1) return `db.${collection}.deleteOne({ _id: ${ids[0]} })`;
  const shown = ids.slice(0, 2).join(", ");
  const more = ids.length > 2 ? `, … ${ids.length - 2} more` : "";
  return `db.${collection}.deleteMany({ _id: { $in: [${shown}${more}] } })`;
}

/**
 * Asks before deleting documents: which server and collection, which
 * documents (or how many the filter matches), and the exact statement.
 * Cancel has the focus. Deleting by filter asks for the collection's name.
 */
export function DeleteDocumentsDialog() {
  const pending = useDeleteStore((s) => s.pending);
  if (!pending) return null;
  return <DeleteDialog key={pending.kind + pending.tabId} request={pending} />;
}

function DeleteDialog({ request }: { request: DeleteRequest }) {
  const running = useDeleteStore((s) => s.running);
  const error = useDeleteStore((s) => s.error);
  const tab = useSessionsStore((s) => s.tabs.find((t) => t.id === request.tabId));
  const color = useConnectionColor(tab?.connection.id ?? "");
  const [typed, setTyped] = useState("");
  if (!tab || tab.kind !== TabKind.Collection) return null;

  const byFilter = request.kind === "filter";
  const everything = byFilter && Object.keys(request.filter).length === 0;
  const count = byFilter ? request.count : request.documents.length;
  const typedOk = !byFilter || typed.trim() === tab.collection;
  const ready = typedOk && count !== null && count > 0 && !running;
  const title = byFilter
    ? everything
      ? "Delete every document in the collection?"
      : "Delete every document matching the filter?"
    : `Delete ${plural(request.documents.length)}?`;

  return (
    <Modal
      title={title}
      onClose={closeDeleteDialog}
      width="max-w-xl"
      footer={
        <>
          <span className="mr-auto text-xs text-fg-3">
            {byFilter ? "Deleting by filter always asks for the collection's name" : "Deletes on the server, not just on this page"}
          </span>
          <Button autoFocus={!byFilter} onClick={closeDeleteDialog} disabled={running}>
            Cancel
          </Button>
          <Button variant="destructive" disabled={!ready} onClick={() => void confirmDelete()}>
            {running ? <LoaderCircle className="animate-spin" /> : <Trash2 />}
            Delete {count === null ? "" : plural(count)}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-3">
        <p className="m-0 text-fg-2">
          <TriangleAlert aria-hidden className="mr-1.5 inline size-4 align-[-3px] text-danger" />
          From <b className="font-mono font-medium text-fg">{`${tab.database}.${tab.collection}`}</b> on{" "}
          <span className="inline-flex items-center gap-1 align-baseline">
            <ConnectionChip name={tab.connection.name} color={color} size="dot" />
            <b className="font-medium text-fg">{tab.connection.name}</b>
          </span>
          . Deleted documents can't be brought back.
        </p>

        {request.kind === "documents" ? (
          <ul aria-label="Documents to delete" className="m-0 list-none rounded-md border border-line-soft p-0 font-data">
            {request.documents.slice(0, LISTED).map((d, i) => (
              <li key={i} className="flex h-[26px] items-center gap-2.5 border-b border-line-soft px-2.5 whitespace-nowrap last:border-b-0">
                <span className="text-fg">{idText(d._id)}</span>
                <span className="min-w-0 truncate text-fg-3">{documentLabel(d)}</span>
              </li>
            ))}
            {request.documents.length > LISTED && (
              <li className="flex h-[26px] items-center px-2.5 font-sans text-xs text-fg-3">
                and {(request.documents.length - LISTED).toLocaleString("en")} more
              </li>
            )}
          </ul>
        ) : (
          <p className="m-0 flex items-center gap-2 text-sm text-fg-2">
            {request.countError ? (
              <>
                <TriangleAlert className="size-3.5 text-danger" /> Couldn't count them: {request.countError}
              </>
            ) : request.count === null ? (
              <>
                <LoaderCircle className="size-3.5 animate-spin" /> Counting the documents that match…
              </>
            ) : (
              <>
                <CircleCheck className="size-3.5 text-ok" />
                <span>
                  <b className="font-medium text-fg tabular-nums">{request.count.toLocaleString("en")}</b>{" "}
                  {request.count === 1 ? "document matches" : "documents match"}, counted just now
                  {tab.results ? `, not only the ${tab.results.documents.length.toLocaleString("en")} on this page` : ""}.
                </span>
              </>
            )}
          </p>
        )}

        <pre className="m-0 rounded-md border border-line-soft bg-panel px-2.5 py-2 font-data text-[11.5px] leading-relaxed break-all whitespace-pre-wrap text-fg-2">
          {statement(request, tab.collection)}
        </pre>

        {byFilter && (
          <label className="flex flex-col gap-1.5 text-sm text-fg-2">
            <span>
              Type <b className="font-mono font-medium text-fg">{tab.collection}</b> to confirm
            </span>
            <Input
              autoFocus
              value={typed}
              onChange={(e) => setTyped(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && ready) void confirmDelete();
              }}
              spellCheck={false}
              autoComplete="off"
              className="font-mono"
              aria-label="Collection name"
            />
          </label>
        )}

        {error && (
          <p role="alert" className="m-0 flex items-start gap-2 text-sm text-danger">
            <TriangleAlert className="mt-0.5 size-3.5 shrink-0" /> {error}
          </p>
        )}
      </div>
    </Modal>
  );
}
