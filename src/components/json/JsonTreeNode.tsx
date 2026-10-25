import { useContext, useState } from "react";
import { cn } from "@/lib/utils";
import { childEntries } from "../../lib/bsonValue";
import { shortTypeName } from "../../lib/bsonFormat";
import { RowContextMenu } from "../common/ActionMenu";
import { EditableValue } from "./EditableValue";
import { FieldMenuContext } from "./FieldMenuContext";
import { InlineValue } from "./InlineValue";
import { KvRow } from "./KvRow";
import { useLegacyUuid } from "./LegacyUuidContext";
import { ValueEditContext } from "./ValueEditContext";

export interface JsonTreeNodeProps {
  name: string;
  /** An array element: the key shows as a faint [index]. */
  isIndex: boolean;
  value: unknown;
  /** The document edits write to, its `documentKey`, and this field's path in it. */
  doc: unknown;
  docKey: string | null;
  path: string[];
  depth: number;
  /** Nodes shallower than this start expanded; deeper ones start collapsed. */
  defaultOpenDepth: number;
  /** Keep values to one line (tree view) instead of wrapping (inspector). */
  truncate: boolean;
}

/** One field of a document and, while open, its children. */
export function JsonTreeNode(props: JsonTreeNodeProps) {
  const { name, isIndex, value, doc, docKey, path, depth, defaultOpenDepth, truncate } = props;
  const [open, setOpen] = useState(depth < defaultOpenDepth);
  const saved = useContext(ValueEditContext)?.saved ?? null;
  const fieldMenu = useContext(FieldMenuContext);
  const legacyUuid = useLegacyUuid();

  const children = childEntries(value);
  const hasChildren = children !== null && children.length > 0;
  const flashAt =
    saved !== null && docKey !== null && saved.docKey === docKey && saved.path === path.join(".")
      ? saved.at
      : null;

  const row = (
      <KvRow
        depth={depth}
        expanded={hasChildren ? open : undefined}
        onToggle={hasChildren ? () => setOpen((o) => !o) : undefined}
        label={
          isIndex ? (
            <span className="text-fg-3">[{name}]</span>
          ) : (
            <span className="text-json-key">{name}</span>
          )
        }
        type={shortTypeName(value, legacyUuid)}
        valueClassName={truncate ? "truncate" : "break-words whitespace-pre-wrap"}
      >
        <EditableValue
          value={value}
          doc={doc}
          path={path}
          className="group-hover/kv:shadow-[inset_0_0_0_1px_var(--color-line)]"
        >
          {/* keyed by the save, so each save restarts the flash */}
          <span
            key={flashAt ?? "value"}
            className={cn(flashAt !== null && "animate-flash rounded-xs")}
          >
            <InlineValue value={value} wrap={!truncate} />
          </span>
        </EditableValue>
      </KvRow>
  );

  return (
    <>
      {fieldMenu ? (
        <RowContextMenu entries={() => fieldMenu(doc, path)}>
          {/* takes the right-click for the row without boxing it */}
          <div className="contents">{row}</div>
        </RowContextMenu>
      ) : (
        row
      )}
      {open && hasChildren && (
        <div role="group">
          {children.map(([key, child]) => (
            <JsonTreeNode
              key={key}
              {...props}
              name={key}
              isIndex={Array.isArray(value)}
              value={child}
              path={[...path, key]}
              depth={depth + 1}
            />
          ))}
        </div>
      )}
    </>
  );
}
