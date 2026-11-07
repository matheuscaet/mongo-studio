import { memo } from "react";
import { Check, ChevronRight } from "lucide-react";
import { cn } from "@/lib/utils";
import { isPlainObject } from "../../lib/bsonValue";
import type { DocumentColumn } from "../../lib/documentColumns";
import type { RelatedColumn } from "../../lib/relations";
import { DocumentTableCell } from "./DocumentTableCell";
import { RelatedTableCell, type RelatedCellData } from "./RelatedTableCell";

interface DocumentTableRowProps {
  doc: unknown;
  index: number;
  columns: DocumentColumn[];
  selected: boolean;
  /** Picked along with other rows (Ctrl/Cmd- or Shift-click); `selected` is the anchor. */
  picked?: boolean;
  /** Several rows are picked: picked rows show a check, the anchor a stronger tint. */
  multi?: boolean;
  /** Ctrl/Cmd-click toggles the row, Shift-click picks the range from the anchor. */
  onPick?: (index: number, how: "toggle" | "range") => void;
  /** Plays the saved-value flash on the row's cells. */
  flashing: boolean;
  /** Absent in a read-only grid, whose rows can't be selected. */
  onSelect?: (index: number) => void;
  /** Opens or closes the document under the row, in a pane too narrow for the inspector. */
  onToggle?: (index: number) => void;
  /** Whether its document is open under it. */
  expanded?: boolean;
  /** The number shown, when rows are filtered: the document's place in the page. Defaults to index + 1. */
  number?: number;
  /** Related columns after the document's, and this row's aggregates for them. */
  relatedColumns?: RelatedColumn[];
  relatedCells?: RelatedCellData[];
}

/** One document as a grid row, with its sticky row number. */
export const DocumentTableRow = memo(function DocumentTableRow({
  doc,
  index,
  columns,
  selected,
  picked = false,
  multi = false,
  onPick,
  flashing,
  onSelect,
  onToggle,
  expanded = false,
  number = index + 1,
  relatedColumns,
  relatedCells,
}: DocumentTableRowProps) {
  const fields = isPlainObject(doc) ? doc : {};
  const on = selected || picked;
  const tint = multi && selected ? "bg-sel" : "bg-sel-soft";
  const cellBg = on ? tint : "group-hover/row:bg-row-hover";
  return (
    <tr
      data-row={index}
      aria-rowindex={index + 2}
      aria-selected={onSelect ? on : undefined}
      aria-expanded={onToggle ? expanded : undefined}
      className={cn("group/row scroll-mt-[30px]", onSelect && "cursor-pointer", flashing && "*:animate-flash")}
      onMouseDown={
        onPick
          ? (e) => {
              // Shift-click picks rows; it shouldn't also select text
              if (e.shiftKey) e.preventDefault();
            }
          : undefined
      }
      onClick={
        onSelect
          ? (e) => {
              if (onPick && (e.ctrlKey || e.metaKey || e.shiftKey)) {
                onPick(index, e.shiftKey ? "range" : "toggle");
                return;
              }
              onSelect(index);
              // Only the first click of a double-click, which edits a value.
              if (e.detail <= 1) onToggle?.(index);
            }
          : undefined
      }
    >
      <td
        className={cn(
          "sticky left-0 z-[1] h-6 w-11 min-w-11 border-r border-b border-line-soft px-2 text-right",
          onToggle && "cursor-pointer pl-1",
          on ? cn(tint, "text-fg") : "bg-editor text-fg-3 group-hover/row:bg-row-hover",
          expanded && "border-b-transparent",
        )}
        title={onToggle ? (expanded ? "Close the document" : "Open the whole document under this row") : undefined}
      >
        {onToggle && (
          <ChevronRight
            aria-hidden
            className={cn(
              "mr-0.5 inline size-3 align-[-2px] text-fg-3 transition-[transform,opacity] duration-150",
              expanded ? "rotate-90 text-fg opacity-100" : "opacity-0 group-hover/row:opacity-100",
              selected && "opacity-100",
            )}
          />
        )}
        {multi && on && <Check aria-hidden className="mr-1 inline size-3 align-[-2px] text-accent-text" />}
        {number}
      </td>
      {relatedColumns?.map((col, i) => (
        <RelatedTableCell key={col.id} col={col} data={relatedCells?.[i]} className={cellBg} />
      ))}
      {columns.map(({ key }) => (
        <DocumentTableCell key={key} doc={doc} field={key} value={fields[key]} className={cellBg} />
      ))}
    </tr>
  );
});
