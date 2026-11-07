import { useContext } from "react";
import { cn } from "@/lib/utils";
import { documentKey, valueTitle } from "../../lib/bsonFormat";
import { editText, isEditablePath } from "../../lib/valueEdit";
import { openFieldEdit } from "../json/fieldEditStore";
import { ValueEditContext } from "../json/ValueEditContext";
import { useValueEdit } from "../json/useValueEdit";
import { ValueInput } from "../json/ValueInput";
import { useLegacyUuid } from "../json/LegacyUuidContext";
import { BsonCell } from "./BsonCell";

interface DocumentTableCellProps {
  doc: unknown;
  field: string;
  value: unknown;
  /** Background classes, which follow the row's hover and selection. */
  className: string;
}

/** A grid cell: its value, or an in-place editor after a double-click. */
export function DocumentTableCell({ doc, field, value, className }: DocumentTableCellProps) {
  const edit = useValueEdit(value, doc, [field]);
  const legacyUuid = useLegacyUuid();
  // an object, an array or a value with no one-line editor opens as JSON in the document's tree
  const editor = useContext(ValueEditContext);
  const asJson = !edit.editable && value !== undefined && editor !== null && isEditablePath([field]);
  const base = "h-6 max-w-[280px] border-r border-b border-line-soft";

  if (edit.editing && edit.kind !== null) {
    return (
      <td className={cn(base, "relative overflow-visible p-0", className)}>
        <ValueInput
          initial={editText(value, edit.kind)}
          kind={edit.kind}
          field={field}
          variant="cell"
          onCancel={edit.cancel}
          onSave={edit.save}
        />
      </td>
    );
  }

  return (
    <td
      className={cn(
        base,
        "overflow-hidden px-2.5 text-ellipsis whitespace-nowrap",
        (edit.editable || asJson) && "cursor-text",
        className,
      )}
      title={value === undefined ? undefined : valueTitle(value, legacyUuid)}
      onDoubleClick={
        edit.editable
          ? (e) => {
              e.stopPropagation();
              edit.begin();
            }
          : asJson
            ? () => {
                const key = documentKey(doc);
                window.getSelection()?.removeAllRanges();
                if (key !== null) openFieldEdit(key, [field], "json");
              }
            : undefined
      }
    >
      <BsonCell value={value} />
    </td>
  );
}
