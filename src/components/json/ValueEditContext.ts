import { createContext } from "react";
import type { FieldEdit } from "../../types/query";

/** The value an edit last wrote, so the rows showing it can flash. */
export interface SavedValue {
  /** `documentKey` of the document written. */
  docKey: string;
  /** Dotted field path within it. */
  path: string;
  /** When it was saved; a new save of the same field restarts the flash. */
  at: number;
}

export interface ValueEditor {
  /** Writes `value` (Extended JSON) at `path` in `doc`; rejects with a message. */
  commit: (doc: unknown, path: string[], value: unknown) => Promise<void>;
  /**
   * Writes a whole value - an object, an array, a new field - and offers to
   * undo it: `previous` (canonical Extended JSON) goes back, or, without
   * it, the field goes away.
   */
  setValue: (doc: unknown, path: string[], value: unknown, previous?: unknown) => Promise<void>;
  /** Removes, renames or inserts into a field, with an undo. */
  edit: (doc: unknown, edit: FieldEdit) => Promise<void>;
  /** The field as stored, as canonical Extended JSON. */
  readField: (doc: unknown, path: string[]) => Promise<unknown>;
  /** The last successful write, for about as long as its flash lasts. */
  saved: SavedValue | null;
}

/**
 * Present only where values can be written back - Browse results that came
 * from a find. Without it (console output, aggregate results) every value
 * renders read-only.
 */
export const ValueEditContext = createContext<ValueEditor | null>(null);
