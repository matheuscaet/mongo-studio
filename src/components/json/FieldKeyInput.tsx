import { useContext, useEffect, useRef, useState } from "react";
import { cn } from "@/lib/utils";
import { fieldNameError, valueAt } from "../../lib/fieldText";
import { isPlainObject } from "../../lib/bsonValue";
import { closeFieldEdit } from "./fieldEditStore";
import { ValueEditContext } from "./ValueEditContext";

/** A field's name, edited in place: Enter renames ($rename), Esc or leaving cancels. */
export function FieldKeyInput({ doc, path }: { doc: unknown; path: string[] }) {
  const editor = useContext(ValueEditContext);
  const name = path[path.length - 1];
  const [text, setText] = useState(name);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const ref = useRef<HTMLInputElement>(null);

  useEffect(() => {
    ref.current?.focus();
    ref.current?.select();
  }, []);

  async function save() {
    const to = text.trim();
    if (!editor || saving) return;
    if (to === name) {
      closeFieldEdit();
      return;
    }
    const holder = valueAt(doc, path.slice(0, -1));
    const problem =
      fieldNameError(to) ?? (isPlainObject(holder) && to in holder ? `There is already a field named "${to}" here` : null);
    if (problem) {
      setError(problem);
      return;
    }
    setSaving(true);
    try {
      await editor.edit(doc, { kind: "rename", path, to });
      closeFieldEdit();
    } catch (e) {
      setError(String(e));
      setSaving(false);
      ref.current?.focus();
    }
  }

  return (
    <span className="relative inline-flex items-center">
      <input
        ref={ref}
        value={text}
        readOnly={saving}
        spellCheck={false}
        aria-label={`Rename ${path.join(".")}`}
        aria-invalid={error !== null}
        title={error ?? "Enter renames · Esc cancels"}
        size={Math.max(8, text.length + 1)}
        className={cn(
          "h-5 rounded-xs bg-field px-1 font-data text-json-key outline-none",
          error ? "shadow-[inset_0_0_0_2px_var(--color-danger)]" : "shadow-[inset_0_0_0_2px_var(--color-accent)]",
        )}
        onChange={(e) => {
          setText(e.target.value);
          setError(null);
        }}
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === "Escape") {
            e.preventDefault();
            closeFieldEdit();
          } else if (e.key === "Enter") {
            e.preventDefault();
            void save();
          }
        }}
        onBlur={() => {
          if (!saving) closeFieldEdit();
        }}
        onClick={(e) => e.stopPropagation()}
        onDoubleClick={(e) => e.stopPropagation()}
      />
      {error && (
        <span
          role="alert"
          className="absolute top-full left-0 z-20 mt-0.5 w-max max-w-[280px] rounded-md border border-line bg-popover px-2 py-1 font-sans text-xs whitespace-normal text-danger shadow-overlay"
        >
          {error}
        </span>
      )}
    </span>
  );
}
