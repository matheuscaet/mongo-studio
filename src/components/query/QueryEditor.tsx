import { useEffect, useRef } from "react";
import Editor from "@monaco-editor/react";
import type * as Monaco from "monaco-editor";
import { useThemeStore } from "../../store/themeStore";
import { isLightTheme } from "../../lib/themes";
import { attachCompletion } from "../../lib/monacoCompletion";
import { addEditorCommand } from "../../lib/monaco";
import type { CompletionContext } from "../../lib/monacoCompletion";

const LINE_HEIGHT = 18;

interface QueryEditorProps {
  value: string;
  onChange: (value: string) => void;
  /** What the text is, which decides what completion offers. */
  kind: "filter" | "sort" | "pipeline";
  /** Read when completion runs, so it always sees the current collection. */
  completionContext: () => CompletionContext | null;
  /** Runs the query: Enter on one line, Ctrl/Cmd+Enter on several. */
  onSubmit: () => void;
  placeholder?: string;
  multiline?: boolean;
  ariaLabel: string;
}

/**
 * A JSON editor sized like a form field: syntax colours, invalid JSON
 * underlined, and MongoDB completion (fields, operators, stages).
 */
export function QueryEditor({
  value,
  onChange,
  kind,
  completionContext,
  onSubmit,
  placeholder,
  multiline = false,
  ariaLabel,
}: QueryEditorProps) {
  const themeId = useThemeStore((s) => s.themeId);
  // Monaco binds commands and providers once, at mount; refs let them reach
  // the latest callbacks instead of the first render's.
  const submitRef = useRef(onSubmit);
  const contextRef = useRef(completionContext);
  submitRef.current = onSubmit;
  contextRef.current = completionContext;
  const detach = useRef<(() => void) | null>(null);

  useEffect(() => () => detach.current?.(), []);

  function handleMount(editor: Monaco.editor.IStandaloneCodeEditor, monaco: typeof Monaco) {
    const model = editor.getModel();
    if (model) {
      detach.current = attachCompletion(model, {
        editor: kind,
        context: () => contextRef.current(),
      });
    }

    if (multiline) {
      addEditorCommand(editor, monaco.KeyMod.CtrlCmd | monaco.KeyCode.Enter, () =>
        submitRef.current(),
      );
      return;
    }
    // One line: Enter runs the query - unless the suggestion list is open,
    // where it accepts the highlighted item as usual.
    addEditorCommand(
      editor,
      monaco.KeyCode.Enter,
      () => submitRef.current(),
      "!suggestWidgetVisible",
    );
    // Pasted multi-line JSON would hide everything past its first line.
    editor.onDidPaste(() => {
      if (!model || model.getLineCount() === 1) return;
      const flat = model.getValue().replace(/\s*\r?\n\s*/g, " ");
      model.setValue(flat);
      editor.setPosition({ lineNumber: 1, column: flat.length + 1 });
    });
  }

  return (
    <div
      className={`overflow-hidden rounded border border-border-subtle bg-panel focus-within:border-accent ${
        multiline ? "h-20 resize-y" : ""
      }`}
      style={multiline ? undefined : { height: LINE_HEIGHT + 8 }}
    >
      <Editor
        language="json"
        theme={isLightTheme(themeId) ? "light" : "vs-dark"}
        value={value}
        onChange={(next) => onChange(next ?? "")}
        onMount={handleMount}
        options={{
          ariaLabel,
          placeholder,
          fontSize: 12,
          lineHeight: LINE_HEIGHT,
          padding: { top: 4, bottom: 4 },
          minimap: { enabled: false },
          lineNumbers: "off",
          glyphMargin: false,
          folding: false,
          lineDecorationsWidth: 6,
          lineNumbersMinChars: 0,
          renderLineHighlight: "none",
          overviewRulerLanes: 0,
          overviewRulerBorder: false,
          hideCursorInOverviewRuler: true,
          scrollBeyondLastLine: false,
          scrollbar: multiline
            ? { verticalScrollbarSize: 8, horizontalScrollbarSize: 8 }
            : { vertical: "hidden", horizontal: "hidden", alwaysConsumeMouseWheel: false },
          wordWrap: multiline ? "on" : "off",
          automaticLayout: true,
          // the suggestion list must escape this small box instead of being clipped
          fixedOverflowWidgets: true,
          wordBasedSuggestions: "off",
          quickSuggestions: { other: true, strings: true, comments: false },
          tabSize: 2,
        }}
      />
    </div>
  );
}
