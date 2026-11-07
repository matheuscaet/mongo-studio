export interface FindQueryInput {
  filter: unknown;
  sort: unknown | null;
  projection: unknown | null;
  limit: number | null;
  skip: number | null;
}

export interface QueryResultPage {
  documents: unknown[];
  returned: number;
}

export interface IndexInfo {
  name: string;
  key: unknown;
  unique: boolean;
}

export interface CollectionStats {
  documentCount: number;
  indexes: IndexInfo[];
}

/** A change to a document's shape (see src-tauri/src/field_edit.rs). */
export type FieldEdit =
  /** `expected`: the value as the user saw it; anything else there now isn't removed. */
  | { kind: "unset"; path: string[]; expected?: unknown }
  | { kind: "rename"; path: string[]; to: string }
  | { kind: "insert"; path: string[]; value: unknown; position?: number | null };

export interface FieldEditResult {
  /** The document as stored afterwards. */
  document: unknown;
  /** What the edit removed, as canonical Extended JSON, for an exact undo. */
  previous: unknown | null;
}
