import { useContext, useEffect, useRef, useState } from "react";
import type { KeyboardEvent } from "react";
import { cn } from "@/lib/utils";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { fieldNameError, newObjectId, valueAt } from "../../lib/fieldText";
import { isPlainObject } from "../../lib/bsonValue";
import { parseEdit, type EditKind } from "../../lib/valueEdit";
import { closeFieldEdit } from "./fieldEditStore";
import { KvRow } from "./KvRow";
import { ValueEditContext } from "./ValueEditContext";

/** The types a new value can start as, and how each reads its text. */
const TYPES = [
  { name: "String", kind: "string", hint: "text" },
  { name: "Number", kind: "number", hint: "0" },
  { name: "Long", kind: "long", hint: "a whole number" },
  { name: "Decimal", kind: "decimal", hint: "10.25" },
  { name: "Boolean", kind: "boolean", hint: "true or false" },
  { name: "Date", kind: "date", hint: "2026-11-06T10:18:00Z" },
  { name: "ObjectId", kind: "objectId", hint: "24 hex characters, or empty for a new one" },
  { name: "Null", fixed: null, shown: "null" },
  { name: "Array", fixed: [], shown: "[ ]" },
  { name: "Object", fixed: {}, shown: "{ }" },
] as const;
type TypeName = (typeof TYPES)[number]["name"];

interface NewFieldRowProps {
  doc: unknown;
  /** The object or array the value goes into; `[]` is the document itself. */
  path: string[];
  depth: number;
}

/**
 * A row for a new field (in an object) or item (at the end of an array):
 * name, type and value. Enter adds it ($set, or $push), Esc cancels.
 */
export function NewFieldRow({ doc, path, depth }: NewFieldRowProps) {
  const editor = useContext(ValueEditContext);
  const holder = valueAt(doc, path);
  const isArray = Array.isArray(holder);
  const [name, setName] = useState("");
  const [type, setType] = useState<TypeName>("String");
  const [text, setText] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const first = useRef<HTMLInputElement>(null);
  const spec = TYPES.find((t) => t.name === type) ?? TYPES[0];

  useEffect(() => {
    first.current?.focus();
    first.current?.scrollIntoView({ block: "nearest" });
  }, []);

  async function save() {
    if (!editor || saving) return;
    const key = name.trim();
    if (!isArray) {
      const problem =
        fieldNameError(key) ?? (isPlainObject(holder) && key in holder ? `There is already a field named "${key}" here` : null);
      if (problem) {
        setError(problem);
        return;
      }
    }
    let value: unknown;
    if ("fixed" in spec) value = spec.fixed;
    else if (spec.kind === "objectId" && text.trim() === "") value = { $oid: newObjectId() };
    else {
      const parsed = parseEdit(text, spec.kind as EditKind);
      if (!parsed.ok) {
        setError(parsed.error);
        return;
      }
      value = parsed.value;
    }
    setSaving(true);
    setError(null);
    try {
      if (isArray) await editor.edit(doc, { kind: "insert", path, value });
      else await editor.setValue(doc, [...path, key], value);
      closeFieldEdit();
    } catch (e) {
      setError(String(e));
      setSaving(false);
    }
  }

  const onKeyDown = (e: KeyboardEvent) => {
    e.stopPropagation();
    if (e.key === "Escape") {
      e.preventDefault();
      closeFieldEdit();
    } else if (e.key === "Enter") {
      e.preventDefault();
      void save();
    }
  };
  const field = cn(
    "h-5 rounded-xs bg-field px-1 font-data text-fg outline-none placeholder:text-fg-3",
    "shadow-[inset_0_0_0_1px_var(--color-line)] focus:shadow-[inset_0_0_0_2px_var(--color-accent)]",
  );
  const target = isArray ? path.join(".") : [...path, name.trim() || "‹name›"].join(".");

  return (
    <div onClick={(e) => e.stopPropagation()} onDoubleClick={(e) => e.stopPropagation()}>
      <div className="bg-[color-mix(in_srgb,var(--color-accent)_7%,transparent)]">
        <KvRow
          depth={depth}
          aria-label={isArray ? "New item" : "New field"}
          label={
            isArray ? (
              <span className="text-fg-3">[{Array.isArray(holder) ? holder.length : 0}]</span>
            ) : (
              <input
                ref={first}
                value={name}
                readOnly={saving}
                placeholder="field name"
                aria-label="New field name"
                spellCheck={false}
                size={Math.max(10, name.length + 1)}
                className={cn(field, "text-json-key")}
                onChange={(e) => {
                  setName(e.target.value);
                  setError(null);
                }}
                onKeyDown={onKeyDown}
              />
            )
          }
          type={
            <Select
              value={type}
              onValueChange={(v) => {
                setType(v as TypeName);
                setError(null);
              }}
            >
              <SelectTrigger size="sm" aria-label="Type of the new value" className="h-5 gap-1 px-1.5 text-xs [&_svg]:size-3">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {TYPES.map((t) => (
                  <SelectItem key={t.name} value={t.name}>
                    {t.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          }
        >
          {"fixed" in spec ? (
            <span className="text-json-punct">{spec.shown}</span>
          ) : (
            <input
              ref={isArray ? first : undefined}
              value={text}
              readOnly={saving}
              placeholder={spec.hint}
              aria-label="New value"
              spellCheck={false}
              className={cn(field, "w-full")}
              onChange={(e) => {
                setText(e.target.value);
                setError(null);
              }}
              onKeyDown={onKeyDown}
            />
          )}
        </KvRow>
      </div>
      <p
        className="m-0 truncate pt-0.5 pr-2.5 pb-1.5 font-sans text-xs text-fg-3"
        style={{ paddingLeft: 28 + depth * 16 }}
        role={error ? "alert" : undefined}
      >
        {error ? (
          <span className="text-danger">{error}</span>
        ) : (
          <>
            Enter adds · Esc cancels ·{" "}
            <code className="font-mono text-fg-2">
              {isArray ? `$push: { "${target}": … }` : `$set: { "${target}": … }`}
            </code>
          </>
        )}
      </p>
    </div>
  );
}
