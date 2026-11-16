import type * as Monaco from "monaco-editor";
import { suggest } from "./mongoCompletion";
import type { CompletionEditor, SuggestionKind } from "./mongoCompletion";
import { completionSource } from "./completionData";
import { QUERY_LANGUAGE } from "./monacoQueryLanguage";

/** Where an editor's completions look names up, read when completion runs. */
export interface CompletionContext {
  sessionId: string;
  database: string;
  /** The collection an editor queries; the console reads it from its text. */
  collection: string | null;
}

export interface CompletionTarget {
  editor: CompletionEditor;
  context: () => CompletionContext | null;
}

// Providers are registered per language, for every editor at once, so each
// editor registers its model here to say what kind of text it holds.
const targets = new Map<string, CompletionTarget>();

/** Enables completion for a model; call the returned function on unmount. */
export function attachCompletion(model: Monaco.editor.ITextModel, target: CompletionTarget) {
  targets.set(model.id, target);
  return () => {
    targets.delete(model.id);
  };
}

/**
 * The console runtime's API (src-tauri/src/scripting/prelude.js), for
 * Monaco's TypeScript service: it completes `db.<collection>`, the
 * collection and cursor methods and their options, with signatures and
 * docs. Only what the runtime implements - mongosh-only calls would fail
 * when run.
 */
const CONSOLE_API = `
interface MongoStudioFindOptions {
  projection?: Record<string, unknown>;
  sort?: Record<string, 1 | -1 | { $meta: string }>;
  limit?: number;
  skip?: number;
  /** An index name or key pattern to use. */
  hint?: string | Record<string, unknown>;
  /** Server-side time limit, in milliseconds. */
  maxTimeMS?: number;
  allowDiskUse?: boolean;
  collation?: Record<string, unknown>;
  batchSize?: number;
  comment?: unknown;
}
interface MongoStudioAggregateOptions {
  allowDiskUse?: boolean;
  maxTimeMS?: number;
  hint?: string | Record<string, unknown>;
  collation?: Record<string, unknown>;
  batchSize?: number;
  comment?: unknown;
  let?: Record<string, unknown>;
}
interface MongoStudioWriteOptions {
  /** Insert a document when none matches. */
  upsert?: boolean;
  arrayFilters?: object[];
  hint?: string | Record<string, unknown>;
  collation?: Record<string, unknown>;
  bypassDocumentValidation?: boolean;
  let?: Record<string, unknown>;
  comment?: unknown;
}
interface MongoStudioUpdateResult {
  acknowledged: boolean;
  matchedCount: number;
  modifiedCount: number;
  upsertedCount: number;
  upsertedId: any;
}
/** A query's results, read in batches. Awaiting it directly gives up to 1000 documents. */
interface MongoStudioCursor<T = any> extends PromiseLike<T[]>, AsyncIterable<T> {
  sort(spec: Record<string, 1 | -1 | { $meta: string }>): this;
  limit(n: number): this;
  skip(n: number): this;
  project(projection: Record<string, unknown>): this;
  hint(index: string | Record<string, unknown>): this;
  maxTimeMS(ms: number): this;
  allowDiskUse(allow?: boolean): this;
  collation(collation: Record<string, unknown>): this;
  batchSize(n: number): this;
  comment(comment: unknown): this;
  map<U>(fn: (doc: T) => U): MongoStudioCursor<U>;
  /** Every document. */
  toArray(): Promise<T[]>;
  /** Calls fn for every document; returning false stops. */
  forEach(fn: (doc: T) => unknown): Promise<void>;
  hasNext(): Promise<boolean>;
  next(): Promise<T | null>;
  itcount(): Promise<number>;
  /** Counts what a find matches. */
  count(): Promise<number>;
  explain(verbosity?: "queryPlanner" | "executionStats" | "allPlansExecution"): Promise<any>;
  close(): Promise<void>;
}
type MongoStudioBulkOperation =
  | { insertOne: { document: object } }
  | { updateOne: { filter: object; update: object | object[]; upsert?: boolean; arrayFilters?: object[]; hint?: unknown; collation?: object } }
  | { updateMany: { filter: object; update: object | object[]; upsert?: boolean; arrayFilters?: object[]; hint?: unknown; collation?: object } }
  | { replaceOne: { filter: object; replacement: object; upsert?: boolean; hint?: unknown; collation?: object } }
  | { deleteOne: { filter: object; hint?: unknown; collation?: object } }
  | { deleteMany: { filter: object; hint?: unknown; collation?: object } };
interface MongoStudioCollection {
  /** Documents matching the filter: a cursor (chain sort/limit..., then toArray()). */
  find(filter?: object, projection?: object, options?: MongoStudioFindOptions): MongoStudioCursor;
  /** The first document matching the filter, or null. */
  findOne(filter?: object, projection?: object, options?: MongoStudioFindOptions): Promise<any | null>;
  /** Runs an aggregation pipeline: a cursor. */
  aggregate(pipeline?: object[], options?: MongoStudioAggregateOptions): MongoStudioCursor;
  /** How many documents match the filter (exact). */
  countDocuments(filter?: object, options?: { limit?: number; skip?: number; hint?: unknown; maxTimeMS?: number; collation?: object }): Promise<number>;
  /** The collection's size from its metadata (fast, approximate). */
  estimatedDocumentCount(options?: { maxTimeMS?: number }): Promise<number>;
  /** The distinct values of a field. */
  distinct(field: string, filter?: object, options?: { maxTimeMS?: number; collation?: object }): Promise<any[]>;
  insertOne(document: object, options?: { bypassDocumentValidation?: boolean }): Promise<{ acknowledged: boolean; insertedId: any }>;
  insertMany(documents: object[], options?: { ordered?: boolean; bypassDocumentValidation?: boolean }): Promise<{ acknowledged: boolean; insertedCount: number; insertedIds: any[] }>;
  updateOne(filter: object, update: object | object[], options?: MongoStudioWriteOptions): Promise<MongoStudioUpdateResult>;
  updateMany(filter: object, update: object | object[], options?: MongoStudioWriteOptions): Promise<MongoStudioUpdateResult>;
  replaceOne(filter: object, replacement: object, options?: MongoStudioWriteOptions): Promise<MongoStudioUpdateResult>;
  deleteOne(filter: object, options?: { hint?: unknown; collation?: object }): Promise<{ acknowledged: boolean; deletedCount: number }>;
  deleteMany(filter: object, options?: { hint?: unknown; collation?: object }): Promise<{ acknowledged: boolean; deletedCount: number }>;
  findOneAndUpdate(filter: object, update: object | object[], options?: MongoStudioWriteOptions & { projection?: object; sort?: object; returnDocument?: "before" | "after"; returnNewDocument?: boolean }): Promise<any | null>;
  findOneAndReplace(filter: object, replacement: object, options?: MongoStudioWriteOptions & { projection?: object; sort?: object; returnDocument?: "before" | "after"; returnNewDocument?: boolean }): Promise<any | null>;
  findOneAndDelete(filter: object, options?: { projection?: object; sort?: object; hint?: unknown; collation?: object }): Promise<any | null>;
  /** Many writes in few round trips. */
  bulkWrite(operations: MongoStudioBulkOperation[], options?: { ordered?: boolean; bypassDocumentValidation?: boolean }): Promise<any>;
  getIndexes(): Promise<any[]>;
  createIndex(keys: Record<string, unknown>, options?: Record<string, unknown>): Promise<string>;
  dropIndex(index: string | Record<string, unknown>): Promise<any>;
  /** Drops the collection with its documents and indexes. */
  drop(): Promise<boolean>;
  getName(): string;
  getFullName(): string;
}
interface MongoStudioDb {
  /** A collection of this database; db.<name> works too. */
  getCollection(name: string): MongoStudioCollection;
  /** Same as getCollection(name). */
  collection(name: string): MongoStudioCollection;
  /** This database's name. */
  getName(): string;
  /** Another database on the same server, without switching to it (\`use other\` switches). */
  getSiblingDB(name: string): MongoStudioDb;
  getCollectionNames(): Promise<string[]>;
  getCollectionInfos(filter?: object): Promise<any[]>;
  runCommand(command: object | string): Promise<any>;
  createCollection(name: string, options?: object): Promise<any>;
  /** A database-level aggregation ($currentOp, $documents...). */
  aggregate(pipeline: object[], options?: MongoStudioAggregateOptions): MongoStudioCursor;
}
declare const db: MongoStudioDb & { readonly [collection: string]: MongoStudioCollection };
declare class ObjectId {
  /** A new id, or the one these 24 hex characters spell. */
  constructor(hex?: string);
  toHexString(): string;
  equals(other: unknown): boolean;
  getTimestamp(): Date;
  readonly $oid: string;
}
/** A date (stored as a BSON date). */
declare function ISODate(isoDate?: string): Date;
/** A 64-bit integer. */
declare function NumberLong(value: number | string | bigint): any;
/** A 32-bit integer. */
declare function NumberInt(value: number | string): any;
/** A Decimal128 number. */
declare function NumberDecimal(value: string): any;
/** A UUID (stored as binary subtype 4); a new one without an argument. */
declare function UUID(hex?: string): any;
/** Logs its arguments to the console output. */
declare function print(...values: unknown[]): void;
/** Logs a value as indented JSON. */
declare function printjson(value: unknown): void;
/** Waits; await it. */
declare function sleep(ms: number): Promise<void>;
`;

function itemKind(monaco: typeof Monaco, kind: SuggestionKind) {
  const k = monaco.languages.CompletionItemKind;
  switch (kind) {
    case "collection":
      return k.Module;
    case "field":
      return k.Field;
    case "operator":
      return k.Operator;
    case "stage":
      return k.Keyword;
    case "variable":
      return k.Variable;
  }
}

/** Registers MongoDB completion for the query fields and the script console. */
export function registerMongoCompletion(monaco: typeof Monaco) {
  for (const language of [QUERY_LANGUAGE, "javascript"]) {
    monaco.languages.registerCompletionItemProvider(language, {
      triggerCharacters: ["$", '"', "'"],
      async provideCompletionItems(model, position) {
        const target = targets.get(model.id);
        const context = target?.context();
        if (!target || !context) return { suggestions: [] };

        const textBefore = model.getValueInRange(
          new monaco.Range(1, 1, position.lineNumber, position.column),
        );
        const result = await suggest(
          textBefore,
          target.editor,
          context.collection,
          completionSource(context.sessionId, context.database),
        );
        // a token never spans lines; clamp in case of an unterminated string
        const startColumn = Math.max(1, position.column - result.replaceLength);
        const range = new monaco.Range(
          position.lineNumber,
          startColumn,
          position.lineNumber,
          position.column,
        );
        return {
          suggestions: result.items.map((item) => ({
            label: { label: item.label, description: item.detail },
            kind: itemKind(monaco, item.kind),
            insertText: item.insertText,
            filterText: item.label,
            detail: item.detail,
            documentation: item.documentation,
            sortText: item.sortText,
            range,
          })),
        };
      },
    });
  }

  monaco.typescript.javascriptDefaults.addExtraLib(CONSOLE_API, "mongo-studio-console.d.ts");
}
