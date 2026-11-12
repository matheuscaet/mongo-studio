import { useState } from "react";
import { Trash2, TriangleAlert, Unplug } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogBody,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { useConnectionsStore } from "../../store/connectionsStore";
import {
  CheckboxConnectionTree,
  ConnectionPickerHeader,
  ConnectionTreeEmpty,
  groupState,
  useConnectionTree,
} from "./ConnectionTreePicker";

interface DeleteConnectionsDialogProps {
  /** The connections ticked when it opens. */
  initial: string[];
  onClose: () => void;
}

const plural = (n: number) => `${n} connection${n === 1 ? "" : "s"}`;

/**
 * Picks several saved connections, in the sidebar's folders, and deletes
 * them with their saved passwords. Connected ones are disconnected first.
 */
export function DeleteConnectionsDialog({ initial, onClose }: DeleteConnectionsDialogProps) {
  const [selected, setSelected] = useState(() => new Set(initial));
  const [query, setQuery] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const { profiles, byId, shown, shownIds, filtered } = useConnectionTree(query);
  const sessions = useConnectionsStore((s) => s.sessions);
  const deleteProfiles = useConnectionsStore((s) => s.deleteProfiles);

  const isChecked = (id: string) => selected.has(id);
  // only ids still saved: one deleted elsewhere meanwhile drops out
  const picked = profiles.filter((p) => selected.has(p.id)).map((p) => p.id);
  const connectedCount = picked.filter((id) => sessions[id]).length;

  function setChecked(ids: string[], on: boolean) {
    setSelected((prev) => {
      const next = new Set(prev);
      for (const id of ids) {
        if (on) next.add(id);
        else next.delete(id);
      }
      return next;
    });
  }

  async function handleDelete() {
    setBusy(true);
    setError(null);
    const failed = await deleteProfiles(picked);
    setBusy(false);
    if (failed.length === 0) {
      onClose();
      return;
    }
    // keep the ones that failed ticked, so trying again retries just them
    setSelected(new Set(failed));
    setError(
      `Couldn't delete ${plural(failed.length)}: ${useConnectionsStore.getState().error ?? "unknown error"}`,
    );
  }

  return (
    <Dialog open onOpenChange={(open) => !open && !busy && onClose()}>
      <DialogContent
        aria-describedby={undefined}
        className="grid h-[min(600px,calc(100%-2rem))] w-[min(620px,calc(100%-2rem))] max-w-none grid-rows-[auto_minmax(0,1fr)_auto] sm:max-w-none"
      >
        <DialogHeader>
          <DialogTitle>Delete connections</DialogTitle>
        </DialogHeader>

        <DialogBody className="flex flex-col gap-4 pb-[22px]">
          <p className="m-0 text-sm leading-normal text-fg-2">
            Pick the connections to delete. Their saved passwords are removed too, and this can't be undone.
          </p>
          <div className="-mb-2">
            <ConnectionPickerHeader
              autoFocus
              query={query}
              onQueryChange={setQuery}
              selectAll={{
                state: shownIds.length === 0 ? false : groupState(shownIds, isChecked),
                disabled: shownIds.length === 0 || busy,
                onToggle: () => setChecked(shownIds, groupState(shownIds, isChecked) !== true),
                filtered,
              }}
              count={`${picked.length} of ${profiles.length} selected`}
            />
          </div>
          <div className="rounded-md border border-line p-1">
            {shown.length === 0 ? (
              <ConnectionTreeEmpty hasProfiles={profiles.length > 0} query={query} />
            ) : (
              <CheckboxConnectionTree
                nodes={shown}
                byId={byId}
                isChecked={isChecked}
                setChecked={setChecked}
                connectionTrailing={(p) =>
                  sessions[p.id] && (
                    <span className="inline-flex items-center gap-1 text-xs text-fg-3">
                      <Unplug className="size-3" />
                      connected
                    </span>
                  )
                }
                aria-label="Connections to delete"
              />
            )}
          </div>
          {connectedCount > 0 && (
            <p className="-mt-1.5 flex items-start gap-2 rounded-md border border-warn/30 bg-warn/9 px-2.5 py-2 text-sm leading-[1.45] text-fg-2">
              <TriangleAlert className="mt-px size-3.5 shrink-0 text-warn" />
              <span>
                {connectedCount === 1 ? "1 of them is connected" : `${connectedCount} of them are connected`}.
                Connected ones are disconnected, and their tabs closed, first.
              </span>
            </p>
          )}
        </DialogBody>

        <DialogFooter>
          <span
            role={error ? "alert" : "status"}
            className="flex min-w-0 flex-1 items-center truncate text-sm text-danger"
            title={error ?? undefined}
          >
            {error}
          </span>
          <Button variant="ghost" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button variant="destructive" disabled={busy || picked.length === 0} onClick={() => void handleDelete()}>
            <Trash2 />
            Delete {plural(picked.length)}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
