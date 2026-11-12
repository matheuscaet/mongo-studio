import type { ComponentProps, KeyboardEvent, MouseEvent, ReactNode } from "react";
import { ChevronRight } from "lucide-react";
import { cn } from "@/lib/utils";

/** Indent per tree depth, in px. */
export const TREE_INDENT_PX = 16;

interface TreeRowProps extends Omit<ComponentProps<"div">, "children"> {
  depth: number;
  /** Disclosure state; undefined for a row that can't open (no chevron). */
  expanded?: boolean;
  /** The kind icon or connection chip. */
  icon: ReactNode;
  /** The name, or an inline editor in place of it. */
  label: ReactNode;
  labelClassName?: string;
  /** Status, counts and actions, pushed to the right edge. */
  trailing?: ReactNode;
  selected?: boolean;
  /**
   * Runs on click, and on Enter or Space while the row has focus. An
   * `onClick` that calls `preventDefault` keeps the click from it.
   */
  onActivate?: () => void;
}

/** One 24px explorer row: chevron, icon, label and trailing slot. */
export function TreeRow({
  depth,
  expanded,
  icon,
  label,
  labelClassName,
  trailing,
  selected = false,
  onActivate,
  className,
  style,
  onClick,
  onKeyDown,
  ...rest
}: TreeRowProps) {
  function handleClick(e: MouseEvent<HTMLDivElement>) {
    onClick?.(e);
    if (!e.defaultPrevented) onActivate?.();
  }

  function handleKeyDown(e: KeyboardEvent<HTMLDivElement>) {
    onKeyDown?.(e);
    // only the row itself: keys typed in a button or field inside it are theirs
    if (e.target !== e.currentTarget || (e.key !== "Enter" && e.key !== " ")) return;
    e.preventDefault();
    onActivate?.();
  }

  return (
    <div
      role="treeitem"
      aria-expanded={expanded}
      aria-selected={selected}
      tabIndex={0}
      className={cn(
        "group relative flex h-6 select-none items-center gap-1.5 whitespace-nowrap pr-2 text-base text-fg hover:bg-row-hover",
        selected && "bg-sel hover:bg-sel",
        className,
      )}
      style={{ paddingLeft: 6 + depth * TREE_INDENT_PX, ...style }}
      onClick={handleClick}
      onKeyDown={handleKeyDown}
      {...rest}
    >
      <span className="grid size-4 shrink-0 place-items-center text-fg-3" aria-hidden>
        {expanded !== undefined && (
          <ChevronRight
            className={cn(
              "size-3 transition-transform duration-150 ease-out-expo",
              expanded && "rotate-90",
            )}
          />
        )}
      </span>
      {icon}
      <span className={cn("min-w-0 truncate", labelClassName)}>{label}</span>
      {trailing && (
        <span className="ml-auto flex shrink-0 items-center gap-1.5 pl-1 text-xs text-fg-3">
          {trailing}
        </span>
      )}
    </div>
  );
}
