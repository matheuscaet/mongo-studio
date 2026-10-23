# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

(Desktop app: a Tauri 2 shell around a React web view, shipped for macOS, Windows and Linux. The UI is web technology in a native window, with native file dialogs, OS keychain and menus from Tauri.)

## Users

Backend developers who spend much of their day inside their MongoDB data across several environments (local, staging, production): finding a document, checking why a query behaves as it does, fixing a value, running a quick script, comparing the same collection on two servers.

## Product Purpose

An open-source, cross-platform MongoDB GUI client: browse and query collections, run aggregation pipelines, script against a database, inspect indexes and explain plans, edit documents in place and export results - without installing mongosh or signing into anything. Success is a developer getting to the right document or answer faster than with Compass, NoSQLBooster or a terminal.

## Positioning

- **Light and offline:** a small native app, MIT-licensed, that works fully offline with no account and no telemetry; Monaco and every asset are bundled.
- **Several servers at once:** many live connections side by side, each tab bound to its own connection, so environments can be compared directly.
- **Embedded console:** a mongosh-style JavaScript console backed by an embedded QuickJS runtime - no external mongosh binary.

## Operating Context

Used for long stretches next to an IDE and a terminal, often on a laptop screen. Typical loop: connect to one or more servers, open a database, open collections as tabs, filter/sort/aggregate, read results as a tree or table, double-click to edit a value, open a console on a database for ad-hoc scripts, save scripts to disk, export CSV.

## Capabilities and Constraints

- Connections: URI or host/port form with General/TLS/SSH/Advanced sections; an optional identity color per connection (an automatic one otherwise); secrets in the OS keychain; import/export in Compass's JSON format (selective import); organized in nested folders; menus to connect, disconnect, edit, delete; any number connected at once.
- Explorer and Saved scripts tool windows (resizable); one search box for connections and collections; databases and collections as a tree, collections sorted by name; a Ctrl+K quick open for collections, saved scripts and actions.
- Tabs: collection tabs (Documents, Indexes, Console views, per tab) and console tabs, from any number of connections; each tab shows its collection or console and its connection, underlined in the connection's color.
- Tab groups: tabs gather automatically by connection (a header chip in the connection's color; can be turned off), plus groups the user makes and names, which can mix connections; a group collapses to its chip with a count and can be closed or opened side by side at once.
- Split editor: one tab strip, up to four panes (2 or 3 columns, or a 2x2 grid; Ctrl/Cmd+Alt+1-4, Ctrl/Cmd+\ opens a tab to the side); a clicked tab opens in the focused pane, tabs can be dragged onto a pane, and a pane can be maximized or closed. Panes too narrow for the inspector open a document under its row instead.
- Documents: Find (filter, sort, limit, skip) and Aggregate (pipeline) with MongoDB-aware completion; Explain; results as a document grid with the selected document in an inspector, a JSON tree, or raw JSON; inline value editing; export of the whole query to CSV or JSON.
- Console: Monaco JS editor with completion, run/cancel, Result and Logs, stacked or side-by-side layout with a resizable split, save to a local .js file (Ctrl/Cmd+S), saved scripts in their own tool window, a target pill that moves a console to any database of any connected server, export of the result to CSV or JSON.
- Indexes: list with usage stats.
- Assistant (opt-in): drives an AI coding agent CLI already installed and signed in on the machine - Claude Code or Codex - never an API key of the app's own. A tool window docked right (Ctrl/Cmd+L) holds sessions bound to one database of one connection; Ctrl/Cmd+I asks in place in a filter bar or a console and shows the change as a diff to accept or reject. The agent reads the server only through read-only tools the app serves over a local MCP server (list collections, sample the schema, find and aggregate with a limit, explain, count); its own shell and file tools are off, `$out`/`$merge` are refused, document values need the user's permission unless allowed in settings, and connections are shared one by one (localhost ones start allowed). The user picks the model and reasoning effort per agent - the CLI's own default, one it lists (Codex: the models the account can use; Claude Code: its fable/opus/sonnet/haiku aliases) or any model name - trading cost and speed against strength. It proposes filters, pipelines and console scripts as cards with a read-only dry run; writes come back as scripts that run only when the user presses Run.
- Settings (Ctrl/Cmd+,): theme (dark, light, system), the default result view (each tab and console keeps its own), page size, console engine, import/export, the Assistant, the MCP server.
- Console engines: the built-in one, or mongosh - found on the PATH, picked, or downloaded from MongoDB on request - per console or by default.
- MCP server (opt-in): AI clients on the machine (Claude Code, Cursor, Codex, VS Code) reach the saved connections over a local MCP server with a bearer token; each capability is Off, Ask (an approval card in the app with the statement and a dry-run count) or Allow, each connection None, Read or Read & write; "Add to <client>" writes the entry into that client's config. Separate from the Assistant, which stays read-only.
- Terminology: connection, database, collection, document, filter, sort, pipeline, console, saved script, tab, Assistant, session, proposal, MCP client, grant.

## Brand Commitments

- Name: Mongo Studio. An app icon exists at `src-tauri/icons/icon-source.svg`.
- Appearance: one visual identity in a light and a dark version (DESIGN.md); the earlier editor-style themes were replaced by the redesign.
- UI copy is in English.
- Standing preference (chosen 2026-09-26 in the redesign direction round): the category standard, a dense IDE-style tool, executed at full craft - it should sit comfortably next to DataGrip / JetBrains New UI, VS Code and Linear, whose finish sets the quality bar.

## Evidence on Hand

Sample data lives only in contributors' local dev databases (not in the repo). No customers, testimonials or usage numbers exist; none may be invented.

## Product Principles

1. The data is the interface: documents, fields and types get the space and the clearest type; chrome recedes.
2. Always show where you are: which server, database and collection every tab, query and console runs against.
3. Fast by keyboard, fine by mouse: every frequent action has a shortcut and a visible control.
4. Nothing leaves the machine: no account, no network beyond the user's own servers. The exceptions are opt-in and said plainly where they're turned on: the Assistant, whose messages and what its agent reads go to the agent's provider (Anthropic or OpenAI) under the user's own CLI account; the MCP server, whose clients - and whatever provider each one uses - get what the user's grants let them read; and the mongosh download, which fetches MongoDB's build from downloads.mongodb.com when the user clicks Download.
