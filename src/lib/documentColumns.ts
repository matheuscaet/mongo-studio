import { childEntries, isPlainObject } from "./bsonValue";
import { typeName } from "./bsonFormat";

export interface DocumentColumn {
  /** The top-level field. */
  key: string;
  /** Its BSON type name, taken from the first document that has a value for it. */
  type: string;
}

/**
 * Order: _id, then scalars as stored, then nested fields, then references
 * (other ObjectIds), and last the fields a library keeps for itself, named
 * with two underscores, like Mongoose's `__v`.
 */
function rank(key: string, sample: unknown): number {
  if (key === "_id") return 0;
  if (isInternal(key)) return 4;
  if (typeName(sample) === "ObjectId") return 3;
  if (childEntries(sample) !== null) return 2;
  return 1;
}

/** Whether a library keeps this field for itself, like Mongoose's `__v`. */
function isInternal(key: string): boolean {
  return key.startsWith("__");
}

/**
 * A document's own fields as the inspector and tree list them: _id first,
 * internal fields (`__v`...) last, the rest as stored.
 */
export function orderFields<T>(entries: [string, T][]): [string, T][] {
  const place = (key: string) => (key === "_id" ? 0 : isInternal(key) ? 2 : 1);
  return entries
    .map((entry, i) => ({ entry, i }))
    .sort((a, b) => place(a.entry[0]) - place(b.entry[0]) || a.i - b.i)
    .map(({ entry }) => entry);
}

/** The grid's columns: the union of the documents' top-level fields. */
export function columnsOf(documents: unknown[]): DocumentColumn[] {
  const order: string[] = [];
  const samples = new Map<string, unknown>();
  for (const doc of documents) {
    if (!isPlainObject(doc)) continue;
    for (const [key, value] of Object.entries(doc)) {
      if (!samples.has(key)) {
        order.push(key);
        samples.set(key, value);
      } else if (samples.get(key) === null && value !== null) {
        // a null says nothing about the field's type; keep looking
        samples.set(key, value);
      }
    }
  }
  return order
    .map((key, i) => ({ key, i, rank: rank(key, samples.get(key)) }))
    .sort((a, b) => a.rank - b.rank || a.i - b.i)
    .map(({ key }) => ({ key, type: typeName(samples.get(key)) }));
}
