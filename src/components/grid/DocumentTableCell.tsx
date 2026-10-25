import { cn } from "@/lib/utils";
import { valueTitle } from "../../lib/bsonFormat";
import { editText } from "../../lib/valueEdit";
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
        edit.editable && "cursor-text",
        className,
      )}
      title={value === undefined ? undefined : valueTitle(value, legacyUuid)}
      onDoubleClick={
        edit.editable
          ? (e) => {
              e.stopPropagation();
              edit.begin();
            }
          : undefined
      }
    >
      <BsonCell value={value} />
    </td>
  );
}
