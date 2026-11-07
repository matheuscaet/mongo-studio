import { useCallback, useContext, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { KeyboardEvent, ReactNode } from "react";
import { Link } from "lucide-react";
import { cn } from "@/lib/utils";
import { documentKey } from "../../lib/bsonFormat";
import { columnsOf } from "../../lib/documentColumns";
import { useLegacyUuid } from "../json/LegacyUuidContext";
import { ValueEditContext } from "../json/ValueEditContext";
import { DocumentTableRow } from "./DocumentTableRow";
import { relatedTypeTag, type RelatedGrid } from "./RelatedTableCell";
import { useScrollbarWidth } from "./useScrollbarWidth";
import { useVirtualRows } from "./useVirtualRows";
import { RowContextMenu, type MenuEntry } from "../common/ActionMenu";

const HEADER_HEIGHT = 30;

interface DocumentTableProps {
  documents: unknown[];
  /** Selection, for a grid beside an inspector; without `onSelect` rows can't be selected. */
  selectedIndex?: number | null;
  onSelect?: (index: number) => void;
  /**
   * Several rows: those picked besides the selected one (shown indexes).
   * With `onPick`, Ctrl/Cmd-click toggles a row and Shift-click picks a
   * range; Ctrl+A picks every row, Esc goes back to the selected row alone.
   */
  picked?: ReadonlySet<number>;
  onPick?: (index: number, how: "toggle" | "range") => void;
  onSelectAll?: () => void;
  onClearPicks?: () => void;
  /** The Delete key, on the picked rows. */
  onDelete?: () => void;
  /** A row's right-click menu, built when it opens. */
  rowMenu?: (index: number) => MenuEntry[];
  /** Fade the right edge where the grid meets a panel beside it. */
  fadeRight?: boolean;
  /**
   * A document open under its row, for a pane too narrow for an inspector:
   * which row, how to toggle it, and what to show (given the visible width,
   * so it stays in view while the grid scrolls sideways).
   */
  expandedIndex?: number | null;
  onToggleExpand?: (index: number) => void;
  renderExpanded?: (index: number, width: number) => ReactNode;
  /** The documents the columns come from, when `documents` is a filtered part of them. */
  columnsFrom?: unknown[];
  /** Per row, the number shown (its place in the whole page), when `documents` is filtered. */
  rowNumbers?: number[];
  /** Related columns before the document's, with a totals footer. */
  related?: RelatedGrid;
  "aria-label": string;
  className?: string;
}

/**
 * Documents as a grid: columns inferred from their fields, a sticky header
 * with each field's type, sticky row numbers. With `onSelect`, a click or
 * ArrowUp/ArrowDown selects a row. Scalar cells edit in place on
 * double-click under a ValueEditContext. Large results render only the rows
 * in view (useVirtualRows), so a 2000-document page scrolls like a small one.
 */
export function DocumentTable({
  documents,
  selectedIndex = null,
  onSelect,
  picked,
  onPick,
  onSelectAll,
  onClearPicks,
  onDelete,
  rowMenu,
  fadeRight = false,
  expandedIndex = null,
  onToggleExpand,
  renderExpanded,
  columnsFrom,
  rowNumbers,
  related,
  className,
  ...aria
}: DocumentTableProps) {
  const scroller = useRef<HTMLDivElement>(null);
  const detail = useRef<HTMLTableRowElement>(null);
  const scrollbar = useScrollbarWidth(scroller);
  const columnSource = columnsFrom ?? documents;
  const legacyUuid = useLegacyUuid();
  const columns = useMemo(() => columnsOf(columnSource, legacyUuid), [columnSource, legacyUuid]);
  const relatedCount = related?.columns.length ?? 0;
  const span = columns.length + 1 + relatedCount;
  const saved = useContext(ValueEditContext)?.saved ?? null;
  const expandable = Boolean(onToggleExpand && renderExpanded);
  const open = expandable && expandedIndex !== null && expandedIndex < documents.length ? expandedIndex : null;
  const [detailHeight, setDetailHeight] = useState(0);
  const [viewport, setViewport] = useState(0);
  const rows = useVirtualRows(
    scroller,
    documents.length,
    HEADER_HEIGHT,
    open === null ? null : { index: open, height: detailHeight },
  );
  const { reveal } = rows;

  // The open document's height feeds the row windowing; the grid's visible
  // width keeps the document in view while the grid scrolls sideways.
  useLayoutEffect(() => {
    const el = scroller.current;
    if (!el || !expandable) return;
    // Out of the rendered window the row is gone; its last height stands.
    const measure = () => {
      setViewport(el.clientWidth);
      if (detail.current) setDetailHeight(detail.current.offsetHeight);
    };
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    if (detail.current) observer.observe(detail.current);
    measure();
    return () => observer.disconnect();
  }, [expandable, open]);

  const select = useCallback(
    (index: number) => {
      onSelect?.(index);
      reveal(index);
    },
    [onSelect, reveal],
  );

  const multi = (picked?.size ?? 0) > 1;
  // the row right-clicked last, for the menu that opens on it
  const menuRow = useRef<number | null>(null);

  function onKeyDown(e: KeyboardEvent<HTMLDivElement>) {
    if (!onSelect || documents.length === 0) return;
    if (e.target === e.currentTarget) {
      if (e.key === "Delete" && onDelete) {
        e.preventDefault();
        onDelete();
        return;
      }
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "a" && onSelectAll) {
        e.preventDefault();
        onSelectAll();
        return;
      }
      if (e.key === "Escape" && multi && onClearPicks) {
        e.preventDefault();
        onClearPicks();
        return;
      }
    }
    if (expandable && e.target === e.currentTarget) {
      // Enter or Space opens the selected document under its row; Esc closes it.
      if ((e.key === "Enter" || e.key === " ") && selectedIndex !== null) {
        e.preventDefault();
        onToggleExpand?.(selectedIndex);
        return;
      }
      if (e.key === "Escape" && open !== null) {
        e.preventDefault();
        onToggleExpand?.(open);
        return;
      }
    }
    const last = documents.length - 1;
    const current = selectedIndex ?? -1;
    let next: number | null = null;
    if (e.key === "ArrowDown") next = Math.min(last, current + 1);
    else if (e.key === "ArrowUp") next = Math.max(0, current - 1);
    else if (e.key === "Home") next = 0;
    else if (e.key === "End") next = last;
    if (next === null) return;
    e.preventDefault();
    select(next);
  }

  return (
    <div className={cn("relative flex min-h-0 min-w-0", className)}>
    <div
      ref={scroller}
      tabIndex={onSelect ? 0 : undefined}
      className="relative min-h-0 min-w-0 flex-1 overflow-auto"
      onKeyDown={onKeyDown}
    >
      <table
        role="grid"
        aria-label={aria["aria-label"]}
        aria-rowcount={documents.length + 1}
        className="min-w-full border-separate border-spacing-0 font-data"
      >
        <thead>
          <tr>
            <th
              scope="col"
              className="sticky top-0 left-0 z-[3] h-[30px] w-11 min-w-11 border-r border-b border-r-line-soft border-b-line bg-panel px-2 text-right font-sans text-sm font-medium text-fg-3"
            >
              #
            </th>
            {related?.columns.map((col, i) => (
              <th
                key={col.id}
                scope="col"
                title={related.titles[i]}
                className="sticky top-0 z-[2] h-[30px] max-w-[280px] overflow-hidden border-r border-b border-r-line-soft border-b-line bg-[color-mix(in_srgb,var(--color-accent)_7%,var(--color-panel))] px-2.5 text-left font-sans text-sm font-medium text-ellipsis whitespace-nowrap text-accent-text"
              >
                <Link aria-hidden className="mr-1 inline size-3 align-[-1px]" />
                {related.titles[i]}
                <span className="ml-1.5 font-mono text-2xs font-normal text-fg-3">{relatedTypeTag(col)}</span>
              </th>
            ))}
            {columns.map(({ key, type }) => (
              <th
                key={key}
                scope="col"
                title={`${key}: ${type}`}
                className="sticky top-0 z-[2] h-[30px] max-w-[280px] overflow-hidden border-r border-b border-r-line-soft border-b-line bg-panel px-2.5 text-left font-sans text-sm font-medium text-ellipsis whitespace-nowrap text-fg"
              >
                {key}
                <span className="ml-1.5 font-mono text-2xs font-normal text-fg-3">{type}</span>
              </th>
            ))}
          </tr>
        </thead>
        <MaybeRowMenu
          menu={rowMenu}
          row={menuRow}
          tbody={
        <tbody
          onContextMenu={(e) => {
            const row = (e.target as HTMLElement).closest("tr[data-row]");
            const index = row ? Number((row as HTMLElement).dataset.row) : selectedIndex;
            menuRow.current = index;
            // a row outside the selection becomes the selection, as in a file manager
            if (index !== null && index !== selectedIndex && !picked?.has(index)) onSelect?.(index);
          }}
        >
          {rows.padTop > 0 && (
            <tr aria-hidden style={{ height: rows.padTop }}>
              <td colSpan={span} className="p-0" />
            </tr>
          )}
          {documents.slice(rows.start, rows.end).map((doc, offset) => {
            const i = rows.start + offset;
            const flashing = saved !== null && saved.docKey === documentKey(doc);
            const row = (
              <DocumentTableRow
                // keyed by the save while flashing, so each save restarts it
                key={flashing ? `${i}:${saved.at}` : i}
                doc={doc}
                index={i}
                columns={columns}
                selected={i === selectedIndex}
                picked={picked?.has(i) ?? false}
                multi={multi}
                onPick={onPick}
                flashing={flashing}
                onSelect={onSelect ? select : undefined}
                onToggle={expandable ? onToggleExpand : undefined}
                expanded={i === open}
                number={rowNumbers?.[i]}
                relatedColumns={related?.columns}
                relatedCells={related?.cells[i]}
              />
            );
            if (i !== open) return row;
            return [
              row,
              <tr key="open-document" ref={detail}>
                <td colSpan={span} className="border-b border-line bg-panel p-0">
                  {renderExpanded?.(i, viewport)}
                </td>
              </tr>,
            ];
          })}
          {rows.padBottom > 0 && (
            <tr aria-hidden style={{ height: rows.padBottom }}>
              <td colSpan={span} className="p-0" />
            </tr>
          )}
        </tbody>
          }
        />
        {related && relatedCount > 0 && (
          <tfoot>
            <tr className="[&>td]:sticky [&>td]:bottom-0 [&>td]:h-[26px] [&>td]:border-t [&>td]:border-r [&>td]:border-line [&>td]:border-r-line-soft [&>td]:bg-panel">
              <td className="left-0 z-[3]" />
              {related.columns.map((col, i) => (
                <td
                  key={col.id}
                  className="z-[2] bg-[color-mix(in_srgb,var(--color-accent)_9%,var(--color-panel))]! px-2.5 text-right font-mono font-semibold whitespace-nowrap text-fg"
                >
                  {related.totals[i]}
                </td>
              ))}
              <td colSpan={columns.length} className="z-[2] px-2.5 font-sans text-sm font-medium whitespace-nowrap text-fg-2">
                {related.footer}
                {related.footerNote && <span className="ml-1 text-fg-3">{related.footerNote}</span>}
              </td>
            </tr>
          </tfoot>
        )}
      </table>
    </div>
      {/* Where the grid meets a panel beside it, its right edge fades out
          (an overlay: a CSS mask made every scroll frame repaint the table). */}
      {fadeRight && (
        <div
          aria-hidden
          className="pointer-events-none absolute inset-y-0 w-9 bg-linear-to-r from-transparent to-editor"
          style={{ right: scrollbar }}
        />
      )}
    </div>
  );
}

/** The table body, with a right-click menu for the row under the pointer when there is one. */
function MaybeRowMenu({
  menu,
  row,
  tbody,
}: {
  menu?: (index: number) => MenuEntry[];
  row: { current: number | null };
  tbody: ReactNode;
}) {
  if (!menu) return tbody;
  return (
    <RowContextMenu entries={() => (row.current === null ? [] : menu(row.current))}>{tbody}</RowContextMenu>
  );
}
