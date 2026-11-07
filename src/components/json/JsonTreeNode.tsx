import { useContext, useState } from "react";
import { Ban, Braces, Pencil, Plus, Trash2, Type } from "lucide-react";
import { toast } from "sonner";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { childEntries } from "../../lib/bsonValue";
import { shortTypeName } from "../../lib/bsonFormat";
import { editKindOf, isEditablePath } from "../../lib/valueEdit";
import { RowContextMenu, type MenuEntry } from "../common/ActionMenu";
import { EditableValue } from "./EditableValue";
import { openFieldEdit, useFieldEditMode } from "./fieldEditStore";
import { FieldJsonEditor } from "./FieldJsonEditor";
import { FieldKeyInput } from "./FieldKeyInput";
import { FieldMenuContext } from "./FieldMenuContext";
import { InlineValue } from "./InlineValue";
import { KvRow } from "./KvRow";
import { useLegacyUuid } from "./LegacyUuidContext";
import { NewFieldRow } from "./NewFieldRow";
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
  const editor = useContext(ValueEditContext);
  const saved = editor?.saved ?? null;
  const fieldMenu = useContext(FieldMenuContext);
  const legacyUuid = useLegacyUuid();
  const mode = useFieldEditMode(docKey, path);

  const children = childEntries(value);
  const nested = children !== null;
  const hasChildren = nested && children.length > 0;
  const shown = open || mode === "add";
  const field = path.join(".");
  const flashAt =
    saved !== null && docKey !== null && saved.docKey === docKey && saved.path === field ? saved.at : null;

  // Changing the field itself - its value as a whole, its name, removing
  // it - needs the same writable results as editing a value, and a document
  // to find again (its documentKey).
  const shapeEditable = editor !== null && docKey !== null && isEditablePath(path);
  const scalarEditable = shapeEditable && editKindOf(value) !== null;
  const isArray = Array.isArray(value);
  const what = isIndex ? "item" : "field";

  const open_ = (m: "json" | "rename" | "add") => {
    if (docKey === null) return;
    if (m === "add") setOpen(true);
    openFieldEdit(docKey, path, m);
  };
  const remove = () => {
    editor?.edit(doc, { kind: "unset", path, expected: value }).catch((e) =>
      toast.error(`Couldn't remove ${field}`, { description: String(e) }),
    );
  };
  const empty = () => {
    if (!editor) return;
    editor
      .readField(doc, path)
      .then((previous) => editor.setValue(doc, path, isArray ? [] : {}, previous))
      .catch((e) => toast.error(`Couldn't empty ${field}`, { description: String(e) }));
  };

  const shapeEntries = (): MenuEntry[] =>
    shapeEditable
      ? [
          { label: nested ? "Edit as JSON…" : "Edit value as JSON…", icon: nested ? Braces : Pencil, onSelect: () => open_("json") },
          ...(isIndex ? [] : [{ label: "Rename field…", icon: Type, onSelect: () => open_("rename") }]),
          ...(nested
            ? [
                { label: isArray ? "Add item" : "Add field", icon: Plus, onSelect: () => open_("add") },
                {
                  label: isArray ? "Empty the array  [ ]" : "Empty the object  { }",
                  icon: Ban,
                  disabled: !hasChildren,
                  onSelect: empty,
                },
              ]
            : []),
          { separator: true },
        ]
      : [];
  const removeEntry = (): MenuEntry[] =>
    shapeEditable
      ? [{ separator: true }, { label: isIndex ? "Remove item" : "Remove field", icon: Trash2, danger: true, onSelect: remove }]
      : [];

  const actions =
    shapeEditable && mode === null ? (
      <>
        {nested && (
          <Button
            variant="ghost"
            size="icon-xs"
            title={isArray ? "Add item" : "Add field"}
            aria-label={`${isArray ? "Add an item to" : "Add a field to"} ${field}`}
            onClick={(e) => {
              e.stopPropagation();
              open_("add");
            }}
          >
            <Plus />
          </Button>
        )}
        <Button
          variant="ghost"
          size="icon-xs"
          title="Edit as JSON"
          aria-label={`Edit ${field} as JSON`}
          onClick={(e) => {
            e.stopPropagation();
            open_("json");
          }}
        >
          <Braces />
        </Button>
        <Button
          variant="ghost"
          size="icon-xs"
          title={`Remove ${what}`}
          aria-label={`Remove ${field}`}
          className="hover:text-danger"
          onClick={(e) => {
            e.stopPropagation();
            remove();
          }}
        >
          <Trash2 />
        </Button>
      </>
    ) : undefined;

  const valueView = (
    // keyed by the save, so each save restarts the flash
    <span key={flashAt ?? "value"} className={cn(flashAt !== null && "animate-flash rounded-xs")}>
      <InlineValue value={value} wrap={!truncate} />
    </span>
  );

  const row = (
    <KvRow
      depth={depth}
      expanded={hasChildren ? shown : undefined}
      onToggle={hasChildren ? () => setOpen((o) => !o) : undefined}
      label={
        mode === "rename" ? (
          <FieldKeyInput doc={doc} path={path} />
        ) : isIndex ? (
          <span className="text-fg-3">[{name}]</span>
        ) : (
          <span
            className={cn("text-json-key", shapeEditable && "-mx-[2px] cursor-text rounded-xs px-[2px] hover:shadow-[inset_0_0_0_1px_var(--color-line)]")}
            title={shapeEditable ? "Double-click to rename" : undefined}
            onDoubleClick={
              shapeEditable
                ? (e) => {
                    e.stopPropagation();
                    window.getSelection()?.removeAllRanges();
                    open_("rename");
                  }
                : undefined
            }
          >
            {name}
          </span>
        )
      }
      type={shortTypeName(value, legacyUuid)}
      actions={actions}
      valueClassName={cn(truncate ? "truncate" : "break-words whitespace-pre-wrap", mode === "json" && "opacity-55")}
    >
      {scalarEditable || !shapeEditable ? (
        <EditableValue
          value={value}
          doc={doc}
          path={path}
          className="group-hover/kv:shadow-[inset_0_0_0_1px_var(--color-line)]"
        >
          {valueView}
        </EditableValue>
      ) : (
        // objects, arrays and values with no one-line editor edit as JSON
        <span
          className="-ml-[3px] cursor-text rounded-xs px-[3px] group-hover/kv:shadow-[inset_0_0_0_1px_var(--color-line)]"
          title="Double-click to edit as JSON"
          onDoubleClick={(e) => {
            e.stopPropagation();
            window.getSelection()?.removeAllRanges();
            open_("json");
          }}
        >
          {valueView}
        </span>
      )}
    </KvRow>
  );

  return (
    <>
      {fieldMenu || shapeEditable ? (
        <RowContextMenu
          entries={() => [
            ...(fieldMenu ? fieldMenu(doc, path).slice(0, 1) : [{ heading: field }]),
            ...shapeEntries(),
            ...(fieldMenu ? fieldMenu(doc, path).slice(1) : []),
            ...removeEntry(),
          ]}
        >
          {/* takes the right-click for the row without boxing it */}
          <div className="contents">{row}</div>
        </RowContextMenu>
      ) : (
        row
      )}
      {mode === "json" ? (
        <FieldJsonEditor doc={doc} path={path} value={value} depth={depth} />
      ) : (
        <>
          {shown && hasChildren && (
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
          {mode === "add" && <NewFieldRow doc={doc} path={path} depth={depth + 1} />}
        </>
      )}
    </>
  );
}

