import { useEffect, useMemo, useState } from "react";
import { toast } from "sonner";
import { api } from "../../lib/tauri";
import { documentKey } from "../../lib/bsonFormat";
import { valueAt } from "../../lib/fieldText";
import { useConnectionsStore } from "../../store/connectionsStore";
import { useSessionsStore } from "../../store/sessionsStore";
import type { CollectionTab } from "../../store/sessionsStore";
import type { FieldEdit } from "../../types/query";
import type { SavedValue, ValueEditor } from "../json/ValueEditContext";

/** How long a saved value flashes (the animate-flash animation). */
const FLASH_MS = 1200;

/** A path as people read it: items[2].sku */
export const pathLabel = (path: string[]) =>
  path.reduce((out, s) => (/^\d+$/.test(s) ? `${out}[${s}]` : out ? `${out}.${s}` : s), "");

const idOf = (doc: unknown) => {
  const id = (doc as { _id?: unknown } | null)?._id;
  if (id === undefined) throw new Error("This document has no _id to update it by");
  return id;
};

/**
 * Writes in-place edits of a tab's results back, then puts the updated
 * document in the results, toasts (with an Undo for changes to a
 * document's shape), and flashes the value. Values are editable only in
 * find results; see CollectionTab.resultsMode.
 */
export function useDocumentValueEditor(tab: CollectionTab): ValueEditor | null {
  const session = useConnectionsStore((s) => s.sessions[tab.connection.id]);
  const replaceDocument = useSessionsStore((s) => s.replaceDocument);
  const [saved, setSaved] = useState<SavedValue | null>(null);

  useEffect(() => {
    if (!saved) return;
    const timer = setTimeout(() => setSaved(null), FLASH_MS);
    return () => clearTimeout(timer);
  }, [saved]);

  const { id: tabId, database, collection, resultsMode } = tab;
  return useMemo<ValueEditor | null>(() => {
    if (!session || resultsMode !== "find") return null;
    const sessionId = session.sessionId;
    const ns = `${database}.${collection}`;

    const show = (updated: unknown, flash: string[] | null) => {
      replaceDocument(tabId, updated);
      if (flash) setSaved({ docKey: documentKey(updated) ?? "", path: flash.join("."), at: Date.now() });
    };
    const set = (id: unknown, path: string[], value: unknown) =>
      api.updateField(sessionId, database, collection, id, path, value);
    const edit = async (id: unknown, change: FieldEdit) =>
      api.editField(sessionId, database, collection, id, change);

    /** Toasts a change with an Undo that runs `undo` and shows its result. */
    const done = (title: string, description: string, undo: () => Promise<{ doc: unknown; flash: string[] | null }>) =>
      toast.success(title, {
        description,
        // long enough to read it and change one's mind
        duration: 8000,
        action: {
          label: "Undo",
          onClick: () => {
            undo()
              .then(({ doc, flash }) => {
                show(doc, flash);
                toast.success("Change undone", { description: `updateOne on ${ns}` });
              })
              .catch((e) => toast.error("Couldn't undo the change", { description: String(e) }));
          },
        },
      });

    return {
      saved,
      commit: async (doc, path, value) => {
        const updated = await set(idOf(doc), path, value);
        show(updated, path);
        toast.success(`${pathLabel(path)} updated`, {
          description: `updateOne on ${ns} · 1 document modified`,
        });
      },

      setValue: async (doc, path, value, previous) => {
        const id = idOf(doc);
        const updated = await set(id, path, value);
        show(updated, path);
        const added = previous === undefined;
        done(`${pathLabel(path)} ${added ? "added" : "updated"}`, `$set ${path.join(".")} · updateOne on ${ns}`, async () =>
          added
            ? { doc: (await edit(id, { kind: "unset", path })).document, flash: null }
            : { doc: await set(id, path, previous), flash: path },
        );
      },

      edit: async (doc, change) => {
        const id = idOf(doc);
        const { path } = change;
        const label = pathLabel(path);
        const result = await edit(id, change);
        if (change.kind === "unset") {
          show(result.document, null);
          const holder = path.slice(0, -1);
          const index = Number(path[path.length - 1]);
          const inArray = holder.length > 0 && Array.isArray(valueAt(doc, holder));
          done(
            `${label} removed`,
            inArray ? `Item ${index + 1} of ${pathLabel(holder)} · updateOne on ${ns}` : `$unset ${path.join(".")} · updateOne on ${ns}`,
            async () =>
              inArray
                ? {
                    doc: (await edit(id, { kind: "insert", path: holder, value: result.previous, position: index })).document,
                    flash: path,
                  }
                : { doc: await set(id, path, result.previous), flash: path },
          );
        } else if (change.kind === "rename") {
          const renamed = [...path.slice(0, -1), change.to];
          show(result.document, renamed);
          done(`${label} renamed to ${change.to}`, `$rename ${path.join(".")} → ${renamed.join(".")} · updateOne on ${ns}`, async () => ({
            doc: (await edit(id, { kind: "rename", path: renamed, to: path[path.length - 1] })).document,
            flash: path,
          }));
        } else {
          const items = valueAt(result.document, path);
          const at = change.position ?? (Array.isArray(items) ? items.length - 1 : 0);
          const item = [...path, String(at)];
          show(result.document, item);
          done(`Item added to ${label}`, `$push ${path.join(".")} · updateOne on ${ns}`, async () => ({
            doc: (await edit(id, { kind: "unset", path: item })).document,
            flash: null,
          }));
        }
      },

      readField: (doc, path) => api.readField(sessionId, database, collection, idOf(doc), path),
    };
  }, [session, resultsMode, saved, database, collection, tabId, replaceDocument]);
}
