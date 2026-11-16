//! What MCP clients changed, kept on this computer: every call that ran and
//! could change data or settings, and every script, with its code, when it
//! ran, where, who approved it, and what came of it. Exported as Markdown
//! (for a ticket or a pull request) or JSON, read back by the get_history
//! tool, and cleared from Settings. One JSON object per line in
//! `mcp-history.jsonl`, in the app's data folder; the oldest entries go
//! past `KEEP`.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

/// Entries kept.
const KEEP: usize = 5000;
/// The file is trimmed back to `KEEP` once it holds this many.
const TRIM_AT: usize = KEEP + KEEP / 5;
/// The code and an error's text are cut here.
const MAX_CODE: usize = 16 * 1024;
const MAX_ERROR: usize = 4 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    /// Milliseconds since the Unix epoch.
    pub at: u64,
    pub client: String,
    pub tool: String,
    #[serde(default)]
    pub connection_id: Option<String>,
    /// Every connection of a call on several.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub connection_ids: Vec<String>,
    /// The connection's name then; several, comma-separated, for a call on
    /// several connections.
    #[serde(default)]
    pub connection_name: Option<String>,
    #[serde(default)]
    pub environment: Option<String>,
    /// `shop.orders`, `shop`, `3 connections`...
    pub target: String,
    /// What ran, as mongosh-style text.
    pub code: String,
    /// ok or error.
    pub outcome: String,
    /// What it changed or returned, in short.
    pub summary: String,
    #[serde(default)]
    pub error: Option<String>,
    /// How it was allowed: "Allowed", "Approved by you"...
    pub approval: String,
    pub duration_ms: u64,
}

/// Which entries to read.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryFilter {
    pub connection_id: Option<String>,
    /// Milliseconds since the Unix epoch.
    pub since: Option<u64>,
    pub limit: Option<usize>,
}

pub(crate) struct History {
    path: PathBuf,
    /// How many lines the file holds, once counted: an append doesn't
    /// read the file again.
    lock: Mutex<Option<usize>>,
}

impl History {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            lock: Mutex::new(None),
        }
    }

    pub fn append(&self, mut entry: HistoryEntry) -> AppResult<()> {
        entry.code = cut(entry.code, MAX_CODE);
        entry.error = entry.error.map(|e| cut(e, MAX_ERROR));
        let line =
            serde_json::to_string(&entry).map_err(|e| AppError::InvalidInput(e.to_string()))?;
        let mut count = self.lock.lock().unwrap();
        let before = match *count {
            Some(n) => n,
            None => self.read_lines()?.len(),
        };
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        writeln!(file, "{line}")?;
        drop(file);
        *count = Some(before + 1);
        if before + 1 >= TRIM_AT {
            let lines = self.read_lines()?;
            let kept = &lines[lines.len().saturating_sub(KEEP)..];
            let tmp = self.path.with_extension("jsonl.tmp");
            fs::write(&tmp, format!("{}\n", kept.join("\n")))?;
            fs::rename(&tmp, &self.path)?;
            *count = Some(kept.len());
        }
        Ok(())
    }

    fn read_lines(&self) -> AppResult<Vec<String>> {
        match fs::read_to_string(&self.path) {
            Ok(text) => Ok(text
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(str::to_string)
                .collect()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e.into()),
        }
    }

    /// The entries the filter picks, newest first. A line that doesn't
    /// read (a newer version's, or damaged) is skipped.
    pub fn list(&self, filter: &HistoryFilter) -> AppResult<Vec<HistoryEntry>> {
        let _guard = self.lock.lock().unwrap();
        let mut entries: Vec<HistoryEntry> = self
            .read_lines()?
            .iter()
            .rev()
            .filter_map(|line| serde_json::from_str(line).ok())
            .filter(|e: &HistoryEntry| {
                filter.connection_id.as_ref().is_none_or(|id| {
                    e.connection_id.as_ref() == Some(id) || e.connection_ids.contains(id)
                }) && filter.since.is_none_or(|since| e.at >= since)
            })
            .collect();
        if let Some(limit) = filter.limit {
            entries.truncate(limit);
        }
        Ok(entries)
    }

    pub fn count(&self) -> AppResult<usize> {
        let mut count = self.lock.lock().unwrap();
        if count.is_none() {
            *count = Some(self.read_lines()?.len());
        }
        Ok(count.unwrap_or(0))
    }

    pub fn clear(&self) -> AppResult<()> {
        let mut count = self.lock.lock().unwrap();
        match fs::remove_file(&self.path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => {
                *count = Some(0);
                Ok(())
            }
        }
    }
}

fn cut(text: String, max: usize) -> String {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n… (cut at {} KB)", &text[..end], max / 1024)
}

/// `2026-11-16 14:03:07 -03:00`, in this computer's time zone.
pub(crate) fn local_time(ms: u64) -> String {
    let at = time::OffsetDateTime::from_unix_timestamp_nanos(ms as i128 * 1_000_000)
        .unwrap_or(time::OffsetDateTime::UNIX_EPOCH)
        .to_offset(crate::logging::local_offset());
    let offset = at.offset();
    let (h, m, _) = offset.as_hms();
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02} {}{:02}:{:02}",
        at.year(),
        u8::from(at.month()),
        at.day(),
        at.hour(),
        at.minute(),
        at.second(),
        if offset.is_negative() { '-' } else { '+' },
        h.unsigned_abs(),
        m.unsigned_abs()
    )
}

fn seconds(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms} ms")
    } else {
        format!("{:.1} s", ms as f64 / 1000.0)
    }
}

/// The entries, oldest first, as Markdown for a ticket or pull request.
pub(crate) fn markdown(entries: &[HistoryEntry]) -> String {
    let mut out = String::from("# Changes made through Mongo Studio's MCP server\n\n");
    if entries.is_empty() {
        out.push_str("Nothing recorded.\n");
        return out;
    }
    let (first, last) = (entries.last().unwrap(), entries.first().unwrap());
    out.push_str(&format!(
        "{} call{}, {} to {}.\n",
        entries.len(),
        if entries.len() == 1 { "" } else { "s" },
        local_time(first.at),
        local_time(last.at)
    ));
    for e in entries.iter().rev() {
        let mut place = e.target.clone();
        if let Some(name) = &e.connection_name {
            if !e.target.contains(name.as_str()) {
                place = format!("{place} on {name}");
            }
        }
        if let Some(env) = &e.environment {
            place.push_str(&format!(" ({env})"));
        }
        out.push_str(&format!(
            "\n## {} · {} · {place}\n\n",
            local_time(e.at),
            e.tool
        ));
        out.push_str(&format!(
            "- Client: {} · {} · took {}\n",
            e.client,
            e.approval,
            seconds(e.duration_ms)
        ));
        match &e.error {
            Some(error) => out.push_str(&format!(
                "- Result: **failed** · {}\n",
                error.replace('\n', " ")
            )),
            None => out.push_str(&format!("- Result: ok · {}\n", e.summary)),
        }
        if !e.code.is_empty() {
            let fence = if e.code.contains("```") {
                "````"
            } else {
                "```"
            };
            out.push_str(&format!("\n{fence}js\n{}\n{fence}\n", e.code.trim_end()));
        }
    }
    out
}

/// The entries, oldest first, as JSON with readable times.
pub(crate) fn json(entries: &[HistoryEntry]) -> String {
    let items: Vec<serde_json::Value> = entries
        .iter()
        .rev()
        .map(|e| {
            let mut value = serde_json::to_value(e).unwrap_or_default();
            if let Some(map) = value.as_object_mut() {
                map.insert("time".into(), serde_json::Value::String(local_time(e.at)));
            }
            value
        })
        .collect();
    serde_json::to_string_pretty(&items).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(at: u64, connection: &str) -> HistoryEntry {
        HistoryEntry {
            at,
            client: "Claude Code".into(),
            tool: "update_documents".into(),
            connection_id: Some(connection.into()),
            connection_ids: Vec::new(),
            connection_name: Some("Billing".into()),
            environment: Some("production".into()),
            target: "shop.orders".into(),
            code: "use shop\ndb.getCollection(\"orders\").updateMany({}, {\"$set\":{\"a\":1}})"
                .into(),
            outcome: "ok".into(),
            summary: "12 documents modified".into(),
            error: None,
            approval: "Approved by you".into(),
            duration_ms: 1200,
        }
    }

    fn history() -> (History, PathBuf) {
        let dir = std::env::temp_dir().join(format!("mcp-history-{}", uuid::Uuid::new_v4()));
        (History::new(dir.join("mcp-history.jsonl")), dir)
    }

    #[test]
    fn entries_are_kept_newest_first_and_filtered() {
        let (history, dir) = history();
        assert!(history.list(&HistoryFilter::default()).unwrap().is_empty());
        history.append(entry(1_000, "c1")).unwrap();
        history.append(entry(2_000, "c2")).unwrap();
        history.append(entry(3_000, "c1")).unwrap();
        let all = history.list(&HistoryFilter::default()).unwrap();
        assert_eq!(
            all.iter().map(|e| e.at).collect::<Vec<_>>(),
            [3_000, 2_000, 1_000]
        );
        let c1 = history
            .list(&HistoryFilter {
                connection_id: Some("c1".into()),
                since: Some(2_000),
                limit: None,
            })
            .unwrap();
        assert_eq!(c1.len(), 1);
        assert_eq!(history.count().unwrap(), 3);
        history.clear().unwrap();
        assert_eq!(history.count().unwrap(), 0);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn the_file_is_trimmed_to_the_newest_entries() {
        let (history, dir) = history();
        for i in 0..TRIM_AT as u64 {
            history.append(entry(i, "c1")).unwrap();
        }
        assert_eq!(history.count().unwrap(), KEEP);
        let newest = history
            .list(&HistoryFilter {
                limit: Some(1),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(newest[0].at, TRIM_AT as u64 - 1);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn markdown_reads_like_a_change_log() {
        let mut failed = entry(2_000, "c1");
        failed.outcome = "error".into();
        failed.error = Some("E11000 duplicate key".into());
        let text = markdown(&[failed, entry(1_000, "c1")]);
        assert!(text.starts_with("# Changes made through Mongo Studio's MCP server\n\n2 calls"));
        let first = text.find("Result: ok").unwrap();
        let second = text.find("Result: **failed** · E11000").unwrap();
        assert!(first < second, "oldest first:\n{text}");
        assert!(text.contains("update_documents · shop.orders on Billing (production)"));
        assert!(text.contains("```js\nuse shop\n"));
        let json = json(&[entry(1_000, "c1")]);
        assert!(json.contains("\"time\""));
    }
}
