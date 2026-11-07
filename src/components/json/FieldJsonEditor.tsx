import { useContext, useEffect, useRef, useState } from "react";
import { CircleAlert, LoaderCircle } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Kbd } from "@/components/ui/kbd";
import { fieldText, parseFieldText } from "../../lib/fieldText";
import { isPlainObject } from "../../lib/bsonValue";
import { closeFieldEdit } from "./fieldEditStore";
import { ValueEditContext } from "./ValueEditContext";

interface FieldJsonEditorProps {
  doc: unknown;
  path: string[];
  /** The value as the tree shows it, for its kind and size. */
  value: unknown;
  depth: number;
}

function describe(value: unknown): string {
  if (Array.isArray(value)) return `Array · ${value.length} ${value.length === 1 ? "item" : "items"}`;
  if (isPlainObject(value)) {
    const n = Object.keys(value).length;
    return `Object · ${n} ${n === 1 ? "field" : "fields"}`;
  }
  return "Value";
}

/**
 * A field's whole value as mongosh-style text under its row: read from the
 * server first, so every number keeps its BSON type, and written back with
 * $set. Ctrl+Enter saves, Esc cancels.
 */
export function FieldJsonEditor({ doc, path, value, depth }: FieldJsonEditorProps) {
  const editor = useContext(ValueEditContext);
  const [original, setOriginal] = useState<{ canonical: unknown; text: string } | null>(null);
  const [text, setText] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const box = useRef<HTMLDivElement>(null);
  const area = useRef<HTMLTextAreaElement>(null);
  const field = path.join(".");

  useEffect(() => {
    let live = true;
    editor
      ?.readField(doc, path)
      .then((canonical) => {
        if (!live) return;
        const initial = fieldText(canonical);
        setOriginal({ canonical, text: initial });
        setText(initial);
      })
      .catch((e) => live && setError(String(e)));
    return () => {
      live = false;
    };
    // read once per opening; a save closes the editor
  }, []);

  useEffect(() => {
    if (!original) return;
    box.current?.scrollIntoView({ block: "nearest" });
    area.current?.focus();
    area.current?.select();
  }, [original]);

  async function save() {
    if (!editor || !original || saving) return;
    if (text === original.text) {
      closeFieldEdit();
      return;
    }
    const parsed = parseFieldText(text);
    if (!parsed.ok) {
      setError(parsed.error);
      area.current?.focus();
      return;
    }
    setSaving(true);
    setError(null);
    try {
      await editor.setValue(doc, path, parsed.value, original.canonical);
      closeFieldEdit();
    } catch (e) {
      setError(String(e));
      setSaving(false);
      area.current?.focus();
    }
  }

  const empty = Array.isArray(value) ? "[]" : isPlainObject(value) ? "{}" : null;

  return (
    <div
      ref={box}
      role="group"
      aria-label={`Edit ${field} as JSON`}
      className="my-0.5 mr-2.5 mb-2 overflow-hidden rounded-md border border-accent bg-editor font-sans shadow-[0_0_0_3px_color-mix(in_srgb,var(--color-accent)_16%,transparent)]"
      style={{ marginLeft: 22 + depth * 16 }}
      onClick={(e) => e.stopPropagation()}
      onDoubleClick={(e) => e.stopPropagation()}
    >
      <div className="flex h-[30px] items-center gap-2 border-b border-line-soft bg-panel pr-1 pl-2.5 text-xs text-fg-2">
        <span className="truncate">
          Edit <b className="font-mono font-medium text-fg">{field}</b>
        </span>
        <span className="shrink-0">{describe(value)}</span>
        <span className="flex-1" />
        {empty && (
          <Button
            variant="ghost"
            size="sm"
            disabled={!original || saving}
            onClick={() => {
              setText(empty);
              setError(null);
              area.current?.focus();
            }}
          >
            Set to {empty === "[]" ? "[ ]" : "{ }"}
          </Button>
        )}
      </div>
      {original ? (
        <textarea
          ref={area}
          value={text}
          readOnly={saving}
          spellCheck={false}
          wrap="off"
          aria-label={`Value of ${field}`}
          aria-invalid={error !== null}
          rows={Math.min(12, Math.max(4, text.split("\n").length))}
          className="block max-h-[260px] min-h-24 w-full resize-y border-0 bg-editor px-2.5 py-2 font-data leading-[1.55] text-fg outline-none"
          onChange={(e) => {
            setText(e.target.value);
            setError(null);
          }}
          onKeyDown={(e) => {
            e.stopPropagation();
            if (e.key === "Escape") {
              e.preventDefault();
              closeFieldEdit();
            } else if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
              e.preventDefault();
              void save();
            }
          }}
        />
      ) : (
        !error && (
          <p className="m-0 flex items-center gap-2 px-2.5 py-3 text-xs text-fg-3">
            <LoaderCircle className="size-3.5 animate-spin" /> Reading the value as stored…
          </p>
        )
      )}
      {error && (
        <p role="alert" className="m-0 flex items-start gap-1.5 border-t border-line-soft bg-danger/9 px-2.5 py-1.5 text-xs text-danger">
          <CircleAlert className="mt-px size-3.5 shrink-0" />
          {error}
        </p>
      )}
      <div className="flex h-[30px] items-center gap-2 border-t border-line-soft pr-1 pl-2.5 text-xs text-fg-3">
        <span className="min-w-0 truncate font-mono text-[11px]" title="What Save sends">
          $set: {"{"} "{field}": … {"}"}
        </span>
        <span className="flex-1" />
        <Button variant="ghost" size="sm" onClick={closeFieldEdit} disabled={saving}>
          Cancel
        </Button>
        <Button variant="primary" size="sm" onClick={() => void save()} disabled={!original || saving}>
          {saving && <LoaderCircle className="animate-spin" />}
          Save <Kbd className="ml-0.5">Ctrl ⏎</Kbd>
        </Button>
      </div>
    </div>
  );
}
