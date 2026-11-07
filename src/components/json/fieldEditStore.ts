import { create } from "zustand";

/** How a field is being edited in a tree: its value as text, its name, or a field added under it. */
export type FieldEditMode = "json" | "rename" | "add";

export interface OpenFieldEdit {
  /** `documentKey` of the document. */
  docKey: string;
  /** The field's path; `[]` adds a field to the document itself. */
  path: string[];
  mode: FieldEditMode;
}

/**
 * The one field being edited at a time, app-wide: a grid cell can open its
 * value in the inspector's tree, and opening another edit closes this one.
 */
export const useFieldEditStore = create<{ open: OpenFieldEdit | null }>(() => ({ open: null }));

export function openFieldEdit(docKey: string, path: string[], mode: FieldEditMode) {
  useFieldEditStore.setState({ open: { docKey, path, mode } });
}

export function closeFieldEdit() {
  useFieldEditStore.setState({ open: null });
}

const samePath = (a: string[], b: string[]) => a.length === b.length && a.every((s, i) => s === b[i]);

/** The edit open on this field of this document, if any. */
export function useFieldEditMode(docKey: string | null, path: string[]): FieldEditMode | null {
  return useFieldEditStore((s) =>
    s.open && docKey !== null && s.open.docKey === docKey && samePath(s.open.path, path) ? s.open.mode : null,
  );
}
