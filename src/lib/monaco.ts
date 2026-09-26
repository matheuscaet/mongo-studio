import { loader } from "@monaco-editor/react";
import * as monaco from "monaco-editor";
import EditorWorker from "monaco-editor/editor/editor.worker.js?worker";
import JsonWorker from "monaco-editor/language/json/json.worker.js?worker";
import TsWorker from "monaco-editor/language/typescript/ts.worker.js?worker";
import { registerMongoCompletion } from "./monacoCompletion";

// Bundle Monaco (and its web workers) via Vite instead of letting
// @monaco-editor/react fetch them from a CDN at runtime - this app needs to
// work fully offline against local/private databases. The package's
// `exports` map is `"./*.js": "./esm/vs/*.js"`, so subpaths must omit the
// `esm/vs` prefix (and keep the `.js` extension) or resolution fails.
self.MonacoEnvironment = {
  getWorker(_workerId: string, label: string) {
    if (label === "json") return new JsonWorker();
    if (label === "typescript" || label === "javascript") return new TsWorker();
    return new EditorWorker();
  },
};

loader.config({ monaco });
registerMongoCompletion(monaco);

let ownKeys = 0;

/**
 * `editor.addCommand`, but only for this editor. Monaco keeps keybindings
 * in one service shared by every editor, and its `addCommand` doesn't tie
 * the binding to the editor it's called on: with several editors bound to
 * the same key, the last one registered handles it wherever it's pressed.
 * Every tab's query editors stay mounted, so Enter in one tab's filter ran
 * the newest tab's query. A context key set only in this editor's own
 * scope makes the binding apply only while this editor has focus.
 */
export function addEditorCommand(
  editor: monaco.editor.IStandaloneCodeEditor,
  keybinding: number,
  handler: () => void,
  when?: string,
) {
  const key = `mongoStudioEditor${++ownKeys}`;
  editor.createContextKey(key, true);
  editor.addCommand(keybinding, handler, when ? `${key} && ${when}` : key);
}
