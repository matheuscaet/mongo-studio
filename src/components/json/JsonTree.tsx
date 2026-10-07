import { useMemo } from "react";
import { cn } from "@/lib/utils";
import { childEntries } from "../../lib/bsonValue";
import { orderFields } from "../../lib/documentColumns";
import { documentKey, shortTypeName } from "../../lib/bsonFormat";
import { InlineValue } from "./InlineValue";
import { JsonTreeNode } from "./JsonTreeNode";
import { KvRow } from "./KvRow";

interface JsonTreeProps {
  value: unknown;
  /**
   * The document that edits write to, and `value`'s path in it. Defaults to
   * `value` itself at the root. Values are editable only under a
   * ValueEditContext.
   */
  doc?: unknown;
  path?: string[];
  /** Indent level of the first rows, when nested under a row of its own. */
  depth?: number;
  /** Nodes shallower than this start expanded; deeper ones start collapsed. */
  defaultOpenDepth?: number;
  /** Keep values to one line (tree view) instead of wrapping (inspector). */
  truncate?: boolean;
  /** Accessible name; given, the rows form a tree of their own. */
  "aria-label"?: string;
  className?: string;
}

/**
 * A value as a typed key/value tree: syntax-colored values without their BSON
 * wrappers, a type tag per row, collapsible objects and arrays.
 */
export function JsonTree({
  value,
  doc = value,
  path = [],
  depth = 0,
  defaultOpenDepth = 1,
  truncate = false,
  className,
  ...aria
}: JsonTreeProps) {
  const docKey = useMemo(() => documentKey(doc), [doc]);
  const fields = childEntries(value);
  // a document's own fields: _id first, __v and the like last
  const entries = fields && path.length === 0 && !Array.isArray(value) ? orderFields(fields) : fields;
  const label = aria["aria-label"];

  return (
    <div
      role={label ? "tree" : "group"}
      aria-label={label}
      className={cn("font-data", className)}
    >
      {entries === null || entries.length === 0 ? (
        // a scalar, or an empty object/array: one bare row
        <KvRow depth={depth} type={shortTypeName(value)}>
          <InlineValue value={value} wrap={!truncate} />
        </KvRow>
      ) : (
        entries.map(([key, child]) => (
          <JsonTreeNode
            key={key}
            name={key}
            isIndex={Array.isArray(value)}
            value={child}
            doc={doc}
            docKey={docKey}
            path={[...path, key]}
            depth={depth}
            defaultOpenDepth={defaultOpenDepth}
            truncate={truncate}
          />
        ))
      )}
    </div>
  );
}
