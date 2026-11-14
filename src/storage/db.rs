use crate::fs_util;
use crate::ui::render::slugify;
use anyhow::Result;
use redb::{Database as RedbDb, ReadableDatabase, ReadableTable, TableDefinition};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::system::text_util::tokenize_search_terms;

const TABLE_HISTORY: TableDefinition<u64, &[u8]> = TableDefinition::new("history");
const TABLE_CACHE_KEY_TO_ID: TableDefinition<&str, u64> = TableDefinition::new("cache_key_to_id");
const TABLE_CHAT_SESSIONS: TableDefinition<u64, &[u8]> = TableDefinition::new("chat_sessions");
const TABLE_CHAT_MESSAGES: TableDefinition<u64, &[u8]> = TableDefinition::new("chat_messages");
pub(super) const TABLE_CACHE_PAGES: TableDefinition<&str, &[u8]> =
    TableDefinition::new("cache_pages");
pub(super) const TABLE_CACHE_SEARCH: TableDefinition<&str, &[u8]> =
    TableDefinition::new("cache_search");
pub(super) const TABLE_CACHE_CAPS: TableDefinition<&str, &[u8]> =
    TableDefinition::new("cache_caps");
const TABLE_META: TableDefinition<&str, u64> = TableDefinition::new("meta");

pub struct NewSummaryRecord<'a> {
    pub target_type: &'a str,
    pub title: &'a str,
    pub author: Option<&'a str>,
    pub url: Option<&'a str>,
    pub style: &'a str,
    pub model: &'a str,
    pub word_count: usize,
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    pub duration_ms: u64,
    pub tokens_per_second: Option<f64>,
    pub summary: &'a str,
    pub reasoning: Option<&'a str>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: i64,
    pub target_type: String,
    pub title: String,
    pub author: Option<String>,
    pub url: Option<String>,
    pub style: String,
    pub model: String,
    pub word_count: u32,
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    pub duration_ms: u64,
    pub tokens_per_second: Option<f64>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryItem {
    pub id: i64,
    pub target_type: String,
    pub title: String,
    pub author: Option<String>,
    pub url: Option<String>,
    pub summary: String,
    #[serde(default)]
    pub reasoning: Option<String>,
    pub style: String,
    pub model: String,
    pub word_count: u32,
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    pub duration_ms: u64,
    pub tokens_per_second: Option<f64>,
    pub created_at: String,
}

impl LibraryItem {
    pub fn matches_query(&self, query: &str) -> bool {
        let q = query.trim();
        if q.is_empty() {
            return true;
        }
        let q_low = q.to_lowercase();
        let has = |field: &str| -> bool {
            if q.is_ascii() {
                crate::system::text_util::contains_ascii_case_insensitive(field, q)
            } else {
                field.to_lowercase().contains(&q_low)
            }
        };
        has(&self.title)
            || has(&self.target_type)
            || self.author.as_deref().is_some_and(has)
            || self.url.as_deref().is_some_and(has)
            || has(&self.summary)
    }

    pub fn into_history_entry(self) -> HistoryEntry {
        HistoryEntry {
            id: self.id,
            target_type: self.target_type,
            title: self.title,
            author: self.author,
            url: self.url,
            style: self.style,
            model: self.model,
            word_count: self.word_count,
            prompt_tokens: self.prompt_tokens,
            completion_tokens: self.completion_tokens,
            duration_ms: self.duration_ms,
            tokens_per_second: self.tokens_per_second,
            created_at: self.created_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibrarySearchResult {
    pub id: i64,
    pub target_type: String,
    pub title: String,
    pub author: Option<String>,
    pub url: Option<String>,
    pub snippet: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainStats {
    pub domain: String,
    pub count: i64,
    pub total_words: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelStats {
    pub model: String,
    pub runs: i64,
    pub total_completion_tokens: i64,
    pub avg_tokens_per_second: f64,
    pub p50_tokens_per_second: f64,
    pub p95_tokens_per_second: f64,
    pub avg_duration_ms: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LibraryStats {
    pub total_summaries: i64,
    pub book_summaries: i64,
    pub web_summaries: i64,
    pub file_summaries: i64,
    pub stdin_summaries: i64,
    pub news_summaries: i64,
    pub video_summaries: i64,
    pub total_source_words: i64,
    pub total_summary_words: i64,
    pub total_prompt_tokens: i64,
    pub total_completion_tokens: i64,
    pub avg_tokens_per_second: f64,
    pub max_tokens_per_second: f64,
    pub total_duration_ms: i64,
    pub estimated_minutes_saved: f64,
    pub reads_today: i64,
    pub reads_this_week: i64,
    pub reads_this_month: i64,
    pub active_reading_days: i64,
    pub current_streak_days: i64,
    #[serde(default)]
    pub models: Vec<ModelStats>,
    #[serde(default)]
    pub top_domains: Vec<DomainStats>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatSession {
    pub id: i64,
    pub title: String,
    pub target_type: String,
    pub target_url: Option<String>,
    pub model: String,
    pub message_count: i64,
    pub last_question: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: i64,
    pub session_id: i64,
    pub role: String,
    pub content: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedEntry<T> {
    value: T,
    cached_at: u64,
}

#[derive(Serialize)]
struct SerializedCacheEntry<'a, T: ?Sized> {
    value: &'a T,
    cached_at: u64,
}

#[derive(Deserialize)]
struct CachedAtOnly {
    cached_at: u64,
}

#[derive(Clone)]
pub struct Database {
    db: Arc<RedbDb>,
    path: Option<PathBuf>,
}

fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn format_iso_from_timestamp(unix_secs: u64) -> String {
    let date = format_date_from_timestamp(unix_secs);
    let time_of_day = unix_secs % 86400;
    format!(
        "{}T{:02}:{:02}:{:02}Z",
        date,
        time_of_day / 3600,
        (time_of_day % 3600) / 60,
        time_of_day % 60
    )
}

fn chrono_now_iso() -> String {
    format_iso_from_timestamp(now_unix_secs())
}

fn next_meta_id(write_txn: &redb::WriteTransaction, key: &str) -> Result<u64> {
    let mut meta_table = write_txn.open_table(TABLE_META)?;
    let id = meta_table.get(key)?.map(|v| v.value()).unwrap_or(1);
    meta_table.insert(key, id + 1)?;
    Ok(id)
}

fn extract_domain(url: &str) -> Option<String> {
    let without_scheme = if let Some(rest) = url.strip_prefix("https://") {
        rest
    } else {
        url.strip_prefix("http://")?
    };
    let host = without_scheme.split(&['/', ':', '?', '#'][..]).next()?;
    if host.is_empty() {
        None
    } else {
        Some(host.to_string())
    }
}

pub fn escape_yaml_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                use std::fmt::Write;
                let _ = write!(out, "\\x{:02x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

pub fn format_csv_cell(cell: &str) -> String {
    let trimmed = cell.trim_start();
    let is_numeric = !trimmed.is_empty()
        && cell == trimmed
        && trimmed.parse::<f64>().is_ok_and(|v| v.is_finite());
    let is_formula = !is_numeric
        && (cell.starts_with(['\t', '\r', '=', '+', '-', '@'])
            || trimmed.starts_with(['=', '+', '-', '@']));

    let safe_cell = if is_formula {
        format!("'{}", cell)
    } else {
        cell.to_string()
    };

    if safe_cell.contains(',')
        || safe_cell.contains('"')
        || safe_cell.contains('\n')
        || safe_cell.contains('\r')
    {
        format!("\"{}\"", safe_cell.replace('"', "\"\""))
    } else {
        safe_cell
    }
}

fn prepare_export_file(path: &Path) -> Result<()> {
    if path.is_symlink() {
        anyhow::bail!(
            "Refusing to export to a symbolic link (CWE-59): {}",
            path.display()
        );
    }
    if let Some(parent) = path.parent() {
        fs_util::create_private_dir_all(parent)?;
    }
    Ok(())
}

fn init_redb_schema(db: &RedbDb) -> Result<()> {
    let write_txn = db.begin_write()?;
    {
        let _ = write_txn.open_table(TABLE_HISTORY)?;
        let _ = write_txn.open_table(TABLE_CACHE_KEY_TO_ID)?;
        let _ = write_txn.open_table(TABLE_CHAT_SESSIONS)?;
        let _ = write_txn.open_table(TABLE_CHAT_MESSAGES)?;
        let _ = write_txn.open_table(TABLE_CACHE_PAGES)?;
        let _ = write_txn.open_table(TABLE_CACHE_SEARCH)?;
        let _ = write_txn.open_table(TABLE_CACHE_CAPS)?;
        let _ = write_txn.open_table(TABLE_META)?;
    }
    write_txn.commit()?;
    Ok(())
}

impl Database {
    pub fn default_path() -> PathBuf {
        fs_util::default_data_dir().join("srz.db")
    }

    pub fn open_default() -> Result<Self> {
        Self::open_at(&Self::default_path())
    }

    pub fn open_at(path: &Path) -> Result<Self> {
        if path.is_symlink() {
            anyhow::bail!("Refusing to open database: {:?} is a symlink", path);
        }
        if let Some(parent) = path.parent() {
            fs_util::create_private_dir_all(parent)?;
        }
        let mut last_err = None;
        for attempt in 0..5 {
            match RedbDb::create(path) {
                Ok(db) => {
                    fs_util::set_private_file_perms(path);
                    init_redb_schema(&db)?;
                    return Ok(Self {
                        db: Arc::new(db),
                        path: Some(path.to_path_buf()),
                    });
                }
                Err(e) => {
                    last_err = Some(e);
                    let sleep_ms = 50 * (1 << attempt) + (attempt * 13) % 25;
                    std::thread::sleep(Duration::from_millis(sleep_ms as u64));
                }
            }
        }
        Err(anyhow::anyhow!(
            "Failed to open database at {}: {:?}",
            path.display(),
            last_err
        ))
    }

    #[cfg(test)]
    pub(super) fn open_in_memory() -> Result<Self> {
        let db = redb::Builder::new().create_with_backend(redb::backends::InMemoryBackend::new())?;
        init_redb_schema(&db)?;
        Ok(Self {
            db: Arc::new(db),
            path: None,
        })
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn reset(&self) -> Result<()> {
        let write_txn = self.db.begin_write()?;
        {
            let _ = write_txn.delete_table(TABLE_HISTORY);
            let _ = write_txn.delete_table(TABLE_CACHE_KEY_TO_ID);
            let _ = write_txn.delete_table(TABLE_CHAT_SESSIONS);
            let _ = write_txn.delete_table(TABLE_CHAT_MESSAGES);
            let _ = write_txn.delete_table(TABLE_CACHE_PAGES);
            let _ = write_txn.delete_table(TABLE_CACHE_SEARCH);
            let _ = write_txn.delete_table(TABLE_CACHE_CAPS);
            let _ = write_txn.delete_table(TABLE_META);
        }
        write_txn.commit()?;
        init_redb_schema(&self.db)?;
        Ok(())
    }

    pub fn record_summary(&self, record: &NewSummaryRecord<'_>) -> Result<()> {
        self.record_summary_with_key(record, None)
    }

    pub fn record_summary_with_key(
        &self,
        record: &NewSummaryRecord<'_>,
        cache_key: Option<&str>,
    ) -> Result<()> {
        let write_txn = self.db.begin_write()?;
        let id = next_meta_id(&write_txn, "next_history_id")?;

        let item = LibraryItem {
            id: id as i64,
            target_type: record.target_type.to_string(),
            title: record.title.to_string(),
            author: record.author.map(|s| s.to_string()),
            url: record.url.map(|s| s.to_string()),
            summary: record.summary.to_string(),
            reasoning: record.reasoning.map(|s| s.to_string()),
            style: record.style.to_string(),
            model: record.model.to_string(),
            word_count: record.word_count as u32,
            prompt_tokens: record.prompt_tokens,
            completion_tokens: record.completion_tokens,
            duration_ms: record.duration_ms,
            tokens_per_second: record.tokens_per_second,
            created_at: chrono_now_iso(),
        };

        let bytes = serde_json::to_vec(&item)?;
        {
            let mut hist_table = write_txn.open_table(TABLE_HISTORY)?;
            hist_table.insert(id, bytes.as_slice())?;
        }

        if let Some(ck) = cache_key {
            let mut ck_table = write_txn.open_table(TABLE_CACHE_KEY_TO_ID)?;
            ck_table.insert(ck, id)?;
        }

        write_txn.commit()?;
        Ok(())
    }

    pub fn get_history(&self, limit: usize) -> Result<Vec<HistoryEntry>> {
        let items = self.get_library_items_bounded(limit)?;
        Ok(items
            .into_iter()
            .map(LibraryItem::into_history_entry)
            .collect())
    }

    pub fn get_all_for_index(&self) -> Result<Vec<HistoryEntry>> {
        self.get_history(usize::MAX)
    }

    pub fn get_all_library_items(&self) -> Result<Vec<LibraryItem>> {
        self.get_library_items_bounded(usize::MAX)
    }

    pub fn get_library_items_bounded(&self, limit: usize) -> Result<Vec<LibraryItem>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let read_txn = self.db.begin_read()?;
        let table = match read_txn.open_table(TABLE_HISTORY) {
            Ok(t) => t,
            Err(_) => return Ok(Vec::new()),
        };

        let mut items = Vec::with_capacity(limit.min(128));
        for item_res in table.iter()?.rev() {
            let (_, val) = item_res?;
            let item: LibraryItem = serde_json::from_slice(val.value())?;
            items.push(item);
            if items.len() >= limit {
                break;
            }
        }
        Ok(items)
    }

    pub fn get_library_item_by_id(&self, id: i64) -> Result<Option<LibraryItem>> {
        let read_txn = self.db.begin_read()?;
        let table = match read_txn.open_table(TABLE_HISTORY) {
            Ok(t) => t,
            Err(_) => return Ok(None),
        };

        if let Some(val) = table.get(id as u64)? {
            let item: LibraryItem = serde_json::from_slice(val.value())?;
            return Ok(Some(item));
        }
        Ok(None)
    }

    pub fn get_latest_summary(&self) -> Result<Option<LibraryItem>> {
        let read_txn = self.db.begin_read()?;
        let table = match read_txn.open_table(TABLE_HISTORY) {
            Ok(t) => t,
            Err(_) => return Ok(None),
        };

        if let Some(last_res) = table.iter()?.next_back() {
            let (_, val) = last_res?;
            let item: LibraryItem = serde_json::from_slice(val.value())?;
            return Ok(Some(item));
        }
        Ok(None)
    }

    pub fn delete_library_item(&self, id: i64) -> Result<()> {
        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(TABLE_HISTORY)?;
            table.remove(id as u64)?;
            if let Ok(mut ck_table) = write_txn.open_table(TABLE_CACHE_KEY_TO_ID) {
                let mut keys_to_delete = Vec::new();
                if let Ok(iter) = ck_table.iter() {
                    for (k, v) in iter.flatten() {
                        if v.value() == id as u64 {
                            keys_to_delete.push(k.value().to_string());
                        }
                    }
                }
                for k in keys_to_delete {
                    let _ = ck_table.remove(k.as_str());
                }
            }
        }
        write_txn.commit()?;
        Ok(())
    }

    pub fn create_chat_session(
        &self,
        title: &str,
        target_type: &str,
        target_url: Option<&str>,
        model: &str,
    ) -> Result<i64> {
        let write_txn = self.db.begin_write()?;
        let id = next_meta_id(&write_txn, "next_session_id")?;

        let now = chrono_now_iso();
        let session = ChatSession {
            id: id as i64,
            title: title.to_string(),
            target_type: target_type.to_string(),
            target_url: target_url.map(|s| s.to_string()),
            model: model.to_string(),
            message_count: 0,
            last_question: None,
            created_at: now.clone(),
            updated_at: now,
        };

        let bytes = serde_json::to_vec(&session)?;
        {
            let mut sess_table = write_txn.open_table(TABLE_CHAT_SESSIONS)?;
            sess_table.insert(id, bytes.as_slice())?;
        }

        write_txn.commit()?;
        Ok(id as i64)
    }

    /// Validates that `session_id` fits in `1..u32::MAX` so upper-32-bit key shifts cannot wrap or alias.
    fn chat_session_key_range(session_id: i64) -> Result<(u64, u64, u64)> {
        if session_id <= 0 || session_id >= u32::MAX as i64 {
            anyhow::bail!(
                "Chat session ID out of valid range (1..{}): {}",
                u32::MAX,
                session_id
            );
        }
        let id = session_id as u64;
        let start_key = id << 32;
        let end_key = (id + 1) << 32;
        Ok((id, start_key, end_key))
    }

    pub fn add_chat_turn(&self, session_id: i64, question: &str, answer: &str) -> Result<()> {
        let (sess_u64, start_key, _) = Self::chat_session_key_range(session_id)?;
        let write_txn = self.db.begin_write()?;
        let now = chrono_now_iso();

        let mut next_msg_idx = 1u64;
        {
            let mut sess_table = write_txn.open_table(TABLE_CHAT_SESSIONS)?;
            let updated = if let Some(val) = sess_table.get(sess_u64)? {
                let mut session: ChatSession = serde_json::from_slice(val.value())?;
                next_msg_idx = u64::try_from(session.message_count)
                    .ok()
                    .and_then(|c| c.checked_add(1))
                    .ok_or_else(|| anyhow::anyhow!("Invalid chat session message count"))?;
                if next_msg_idx >= u32::MAX as u64 {
                    anyhow::bail!("Chat session message limit reached");
                }
                session.message_count += 2;
                session.last_question = Some(question.to_string());
                session.updated_at = now.clone();
                Some(serde_json::to_vec(&session)?)
            } else {
                None
            };
            if let Some(bytes) = updated {
                sess_table.insert(sess_u64, bytes.as_slice())?;
            }
        }

        {
            let mut msg_table = write_txn.open_table(TABLE_CHAT_MESSAGES)?;
            let key_q = start_key | next_msg_idx;
            let msg_q = ChatMessage {
                id: next_msg_idx as i64,
                session_id,
                role: "user".to_string(),
                content: question.to_string(),
                created_at: now.clone(),
            };
            msg_table.insert(key_q, serde_json::to_vec(&msg_q)?.as_slice())?;

            let key_a = start_key | (next_msg_idx + 1);
            let msg_a = ChatMessage {
                id: (next_msg_idx + 1) as i64,
                session_id,
                role: "assistant".to_string(),
                content: answer.to_string(),
                created_at: now,
            };
            msg_table.insert(key_a, serde_json::to_vec(&msg_a)?.as_slice())?;
        }

        write_txn.commit()?;
        Ok(())
    }

    pub fn get_chat_sessions(&self, limit: usize) -> Result<Vec<ChatSession>> {
        let read_txn = self.db.begin_read()?;
        let table = match read_txn.open_table(TABLE_CHAT_SESSIONS) {
            Ok(t) => t,
            Err(_) => return Ok(Vec::new()),
        };

        let mut sessions = Vec::new();
        for item_res in table.iter()? {
            let (_, val) = item_res?;
            let session: ChatSession = serde_json::from_slice(val.value())?;
            sessions.push(session);
        }
        sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        sessions.truncate(limit);
        Ok(sessions)
    }

    pub fn get_chat_session(
        &self,
        session_id: i64,
    ) -> Result<Option<(ChatSession, Vec<ChatMessage>)>> {
        let (sess_u64, start_key, end_key) = Self::chat_session_key_range(session_id)?;
        let read_txn = self.db.begin_read()?;
        let sess_table = match read_txn.open_table(TABLE_CHAT_SESSIONS) {
            Ok(t) => t,
            Err(_) => return Ok(None),
        };

        let Some(val) = sess_table.get(sess_u64)? else {
            return Ok(None);
        };
        let session: ChatSession = serde_json::from_slice(val.value())?;

        let mut messages = Vec::new();
        if let Ok(msg_table) = read_txn.open_table(TABLE_CHAT_MESSAGES) {
            for item_res in msg_table.range(start_key..end_key)? {
                let (_, m_val) = item_res?;
                let msg: ChatMessage = serde_json::from_slice(m_val.value())?;
                messages.push(msg);
            }
        }

        Ok(Some((session, messages)))
    }

    pub fn delete_chat_session(&self, session_id: i64) -> Result<bool> {
        let (sess_u64, start_key, end_key) = Self::chat_session_key_range(session_id)?;
        let write_txn = self.db.begin_write()?;
        let removed = {
            let mut sess_table = write_txn.open_table(TABLE_CHAT_SESSIONS)?;
            sess_table.remove(sess_u64)?.is_some()
        };

        if removed {
            let mut msg_table = write_txn.open_table(TABLE_CHAT_MESSAGES)?;
            let keys_to_delete: Vec<u64> = msg_table
                .range(start_key..end_key)?
                .filter_map(|r| r.ok().map(|(k, _)| k.value()))
                .collect();
            for k in keys_to_delete {
                let _ = msg_table.remove(k)?;
            }
        }

        write_txn.commit()?;
        Ok(removed)
    }

    pub fn get_stats(&self) -> Result<LibraryStats> {
        let items = self.get_all_library_items()?;
        let mut stats = LibraryStats {
            total_summaries: items.len() as i64,
            ..Default::default()
        };

        let now_secs = now_unix_secs();
        let today_ymd = format_date_from_timestamp(now_secs);
        let week_ago_ymd = format_date_from_timestamp(now_secs.saturating_sub(7 * 86400));
        let month_ago_ymd = format_date_from_timestamp(now_secs.saturating_sub(30 * 86400));

        let mut model_map: HashMap<String, (i64, i64, f64, i64, Vec<f64>)> = HashMap::new();
        let mut domain_map: HashMap<String, (i64, i64)> = HashMap::new();
        let mut active_days: HashSet<String> = HashSet::new();

        let mut total_tps = 0.0;
        let mut tps_count = 0;

        for item in &items {
            match item.target_type.as_str() {
                "book" => stats.book_summaries += 1,
                "web" => stats.web_summaries += 1,
                "file" => stats.file_summaries += 1,
                "stdin" => stats.stdin_summaries += 1,
                "news" => stats.news_summaries += 1,
                "youtube" => stats.video_summaries += 1,
                _ => {}
            }

            stats.total_source_words += item.word_count as i64;
            stats.total_summary_words += item.summary.split_ascii_whitespace().count() as i64;
            stats.total_prompt_tokens += item.prompt_tokens.unwrap_or(0) as i64;
            stats.total_completion_tokens += item.completion_tokens.unwrap_or(0) as i64;
            stats.total_duration_ms += item.duration_ms as i64;

            if let Some(tps) = item.tokens_per_second
                && tps > 0.0
            {
                total_tps += tps;
                tps_count += 1;
                if tps > stats.max_tokens_per_second {
                    stats.max_tokens_per_second = tps;
                }
            }

            let item_ymd = if item.created_at.len() >= 10 {
                &item.created_at[..10]
            } else {
                ""
            };
            if !item_ymd.is_empty() {
                active_days.insert(item_ymd.to_string());
                if item_ymd == today_ymd {
                    stats.reads_today += 1;
                }
                if item_ymd >= week_ago_ymd.as_str() {
                    stats.reads_this_week += 1;
                }
                if item_ymd >= month_ago_ymd.as_str() {
                    stats.reads_this_month += 1;
                }
            }

            let entry = model_map
                .entry(item.model.clone())
                .or_insert((0, 0, 0.0, 0, Vec::new()));
            entry.0 += 1;
            entry.1 += item.completion_tokens.unwrap_or(0) as i64;
            if let Some(tps) = item.tokens_per_second {
                entry.2 += tps;
                entry.4.push(tps);
            }
            entry.3 += item.duration_ms as i64;

            if let Some(ref u) = item.url
                && let Some(domain) = extract_domain(u)
            {
                let d_entry = domain_map.entry(domain).or_insert((0, 0));
                d_entry.0 += 1;
                d_entry.1 += item.word_count as i64;
            }
        }

        if tps_count > 0 {
            stats.avg_tokens_per_second = total_tps / tps_count as f64;
        }
        stats.active_reading_days = active_days.len() as i64;
        if stats.total_source_words > stats.total_summary_words {
            stats.estimated_minutes_saved =
                (stats.total_source_words - stats.total_summary_words) as f64 / 225.0;
        }

        let mut sorted_days: Vec<String> = active_days.into_iter().collect();
        sorted_days.sort_by(|a, b| b.cmp(a));
        let day_nums: Vec<i64> = sorted_days
            .iter()
            .filter_map(|d| ymd_to_day_number(d))
            .collect();
        let today_num = ymd_to_day_number(&today_ymd).unwrap_or(0);

        if let Some(&most_recent) = day_nums.first()
            && (most_recent == today_num || most_recent == today_num - 1)
        {
            let mut streak = 1;
            for i in 1..day_nums.len() {
                if day_nums[i] == day_nums[i - 1] - 1 {
                    streak += 1;
                } else {
                    break;
                }
            }
            stats.current_streak_days = streak;
        }

        for (model, (runs, completion_tokens, tps_sum, duration_sum, mut tps_vec)) in model_map {
            let avg_tps = if runs > 0 && !tps_vec.is_empty() {
                tps_sum / tps_vec.len() as f64
            } else {
                0.0
            };
            tps_vec.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let p50 = if !tps_vec.is_empty() {
                tps_vec[tps_vec.len() / 2]
            } else {
                0.0
            };
            let p95 = if !tps_vec.is_empty() {
                tps_vec[(tps_vec.len() * 95) / 100]
            } else {
                0.0
            };
            stats.models.push(ModelStats {
                model,
                runs,
                total_completion_tokens: completion_tokens,
                avg_tokens_per_second: avg_tps,
                p50_tokens_per_second: p50,
                p95_tokens_per_second: p95,
                avg_duration_ms: if runs > 0 {
                    (duration_sum as f64) / (runs as f64)
                } else {
                    0.0
                },
            });
        }
        stats.models.sort_by_key(|a| std::cmp::Reverse(a.runs));

        for (domain, (count, total_words)) in domain_map {
            stats.top_domains.push(DomainStats {
                domain,
                count,
                total_words,
            });
        }
        stats
            .top_domains
            .sort_by_key(|a| std::cmp::Reverse(a.count));
        stats.top_domains.truncate(5);

        Ok(stats)
    }

    pub fn search(&self, query: &str) -> Result<Vec<LibrarySearchResult>> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return Ok(Vec::new());
        }
        let query_terms = tokenize_search_terms(trimmed);
        if query_terms.is_empty() {
            return Ok(Vec::new());
        }

        let items = self.get_all_library_items()?;
        let mut results = Vec::new();

        let contains_term = |haystack: &str, needle: &str| -> bool {
            if needle.is_ascii() {
                crate::system::text_util::contains_ascii_case_insensitive(haystack, needle)
            } else {
                haystack.to_lowercase().contains(&needle.to_lowercase())
            }
        };

        for item in items {
            let mut title_matches = 0usize;
            let mut summary_matches = 0usize;

            for term in &query_terms {
                if contains_term(&item.title, term) {
                    title_matches += 1;
                }
                if contains_term(&item.summary, term) {
                    summary_matches += 1;
                }
            }

            if title_matches > 0 || summary_matches > 0 {
                let score = (title_matches as f64 * 10.0) + (summary_matches as f64 * 1.0);
                let snippet = extract_snippet(&item.summary, trimmed, &query_terms);

                results.push(LibrarySearchResult {
                    id: item.id,
                    target_type: item.target_type,
                    title: item.title,
                    author: item.author,
                    url: item.url,
                    snippet,
                    created_at: item.created_at,
                    score: Some(score),
                });
            }
        }

        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.id.cmp(&a.id))
        });

        Ok(results)
    }

    pub fn export_csv(&self, path: &Path) -> Result<()> {
        prepare_export_file(path)?;
        let items = self.get_all_library_items()?;
        let mut csv = String::new();
        csv.push_str("id,target_type,title,author,url,style,model,word_count,prompt_tokens,completion_tokens,duration_ms,tokens_per_second,created_at,summary\n");
        for item in items {
            csv.push_str(&format!(
                "{},{},{},{},{},{},{},{},{},{},{},{},{},{}\n",
                item.id,
                format_csv_cell(&item.target_type),
                format_csv_cell(&item.title),
                format_csv_cell(item.author.as_deref().unwrap_or("")),
                format_csv_cell(item.url.as_deref().unwrap_or("")),
                format_csv_cell(&item.style),
                format_csv_cell(&item.model),
                item.word_count,
                item.prompt_tokens
                    .map(|v| v.to_string())
                    .unwrap_or_default(),
                item.completion_tokens
                    .map(|v| v.to_string())
                    .unwrap_or_default(),
                item.duration_ms,
                item.tokens_per_second
                    .map(|v| format!("{:.1}", v))
                    .unwrap_or_default(),
                format_csv_cell(&item.created_at),
                format_csv_cell(&item.summary),
            ));
        }
        fs_util::write_private_file(path, csv.as_bytes())?;
        Ok(())
    }

    pub fn export_json(&self, path: &Path) -> Result<()> {
        prepare_export_file(path)?;
        let items = self.get_all_library_items()?;
        let json_bytes = serde_json::to_vec_pretty(&items)?;
        fs_util::write_private_file(path, json_bytes)?;
        Ok(())
    }

    pub fn export_markdown_vault(&self, dir: &Path) -> Result<usize> {
        fs_util::create_private_dir_all(dir)?;
        let items = self.get_all_library_items()?;
        let mut written = 0;
        let mut used_filenames: HashSet<String> = HashSet::new();

        for item in items {
            let base_slug = slugify(&item.title);
            let mut filename = format!("{}.md", base_slug);
            let mut counter = 2;
            while used_filenames.contains(&filename) {
                filename = format!("{}-{}.md", base_slug, counter);
                counter += 1;
            }
            used_filenames.insert(filename.clone());

            let file_path = dir.join(&filename);
            let mut content = String::with_capacity(item.summary.len() + 256);
            content.push_str("---\n");
            content.push_str(&format!("title: \"{}\"\n", escape_yaml_string(&item.title)));
            content.push_str(&format!(
                "target_type: \"{}\"\n",
                escape_yaml_string(&item.target_type)
            ));
            if let Some(author) = &item.author {
                content.push_str(&format!("author: \"{}\"\n", escape_yaml_string(author)));
            }
            if let Some(url) = &item.url {
                content.push_str(&format!("url: \"{}\"\n", escape_yaml_string(url)));
            }
            content.push_str(&format!("model: \"{}\"\n", escape_yaml_string(&item.model)));
            content.push_str(&format!("style: \"{}\"\n", escape_yaml_string(&item.style)));
            content.push_str(&format!("word_count: {}\n", item.word_count));
            content.push_str(&format!(
                "created_at: \"{}\"\n",
                escape_yaml_string(&item.created_at)
            ));
            content.push_str("---\n\n");
            let clean_title = item.title.replace(['\n', '\r'], " ");
            content.push_str(&format!("# {}\n\n", clean_title));
            content.push_str(&item.summary);
            content.push('\n');

            fs_util::write_private_file(&file_path, content.as_bytes())?;
            written += 1;
        }

        Ok(written)
    }

    // Unified cache table helpers
    pub fn get_cached_json<T: DeserializeOwned>(
        &self,
        table_def: TableDefinition<&str, &[u8]>,
        key: &str,
        ttl: Duration,
    ) -> Option<T> {
        let read_txn = self.db.begin_read().ok()?;
        let table = read_txn.open_table(table_def).ok()?;
        let val = table.get(key).ok()??;
        let entry: CachedEntry<T> = serde_json::from_slice(val.value()).ok()?;
        if now_unix_secs().saturating_sub(entry.cached_at) > ttl.as_secs() {
            return None;
        }
        Some(entry.value)
    }

    pub fn set_cached_json<T: Serialize + ?Sized>(
        &self,
        table_def: TableDefinition<&str, &[u8]>,
        key: &str,
        value: &T,
    ) -> Result<()> {
        let entry = SerializedCacheEntry {
            value,
            cached_at: now_unix_secs(),
        };
        let bytes = serde_json::to_vec(&entry)?;
        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(table_def)?;
            table.insert(key, bytes.as_slice())?;
        }
        write_txn.commit()?;
        Ok(())
    }

    pub fn remove_cached_entry(
        &self,
        table_def: TableDefinition<&str, &[u8]>,
        key: &str,
    ) -> Result<()> {
        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(table_def)?;
            let _ = table.remove(key)?;
        }
        write_txn.commit()?;
        Ok(())
    }

    pub fn get_cached_summary(
        &self,
        target_type: &str,
        identifier: &str,
        style: &str,
        model: &str,
        ttl: Duration,
    ) -> Option<crate::storage::cache::CachedSummary> {
        let read_txn = self.db.begin_read().ok()?;
        let trimmed_id = identifier.trim();
        let now = now_unix_secs();

        // 1. Direct cache_key lookup
        if let Ok(ck_table) = read_txn.open_table(TABLE_CACHE_KEY_TO_ID)
            && let Ok(Some(id_val)) = ck_table.get(trimmed_id)
        {
            let id = id_val.value();
            if let Ok(hist_table) = read_txn.open_table(TABLE_HISTORY)
                && let Ok(Some(val)) = hist_table.get(id)
                && let Ok(item) = serde_json::from_slice::<LibraryItem>(val.value())
                && item.target_type == target_type
                && item.style == style
                && item.model == model
                && let Some(created_secs) = parse_iso_to_unix_secs(&item.created_at)
                && now.saturating_sub(created_secs) <= ttl.as_secs()
            {
                return Some(crate::storage::cache::CachedSummary {
                    title: item.title,
                    summary: item.summary,
                    word_count: item.word_count as usize,
                    reasoning: item.reasoning,
                });
            }
        }

        None
    }

    pub fn clear_cache_tables(&self) -> Result<()> {
        let write_txn = self.db.begin_write()?;
        {
            let _ = write_txn.delete_table(TABLE_CACHE_PAGES);
            let _ = write_txn.delete_table(TABLE_CACHE_SEARCH);
            let _ = write_txn.delete_table(TABLE_CACHE_CAPS);
            let _ = write_txn.delete_table(TABLE_CACHE_KEY_TO_ID);
        }
        write_txn.commit()?;
        init_redb_schema(&self.db)?;
        Ok(())
    }

    pub fn prune_cache_by_age(&self, max_age: Duration) -> Result<usize> {
        let now = now_unix_secs();
        let max_age_secs = max_age.as_secs();
        let mut pruned = 0;

        fn prune_table(
            table: &mut redb::Table<&str, &[u8]>,
            now: u64,
            max_age: u64,
        ) -> Result<usize> {
            let mut to_delete = Vec::new();
            for item_res in table.iter()? {
                let (k, v) = item_res?;
                if let Ok(rec) = serde_json::from_slice::<CachedAtOnly>(v.value())
                    && now.saturating_sub(rec.cached_at) > max_age
                {
                    to_delete.push(k.value().to_string());
                }
            }
            let count = to_delete.len();
            for k in to_delete {
                let _ = table.remove(k.as_str())?;
            }
            Ok(count)
        }

        let write_txn = self.db.begin_write()?;
        for table_def in [TABLE_CACHE_PAGES, TABLE_CACHE_SEARCH, TABLE_CACHE_CAPS] {
            let mut table = write_txn.open_table(table_def)?;
            pruned += prune_table(&mut table, now, max_age_secs)?;
        }
        write_txn.commit()?;
        Ok(pruned)
    }

    pub fn prune_cache_lru(&self, max_bytes: usize) -> Result<usize> {
        let (total_entries, total_bytes) = self.cache_entry_count_and_bytes();
        if total_bytes <= max_bytes as u64 || total_entries == 0 {
            return Ok(0);
        }

        let mut all_pages: Vec<(String, u64, usize)> = Vec::new();
        {
            let read_txn = self.db.begin_read()?;
            if let Ok(table) = read_txn.open_table(TABLE_CACHE_PAGES) {
                for item_res in table.iter()? {
                    let (k, v) = item_res?;
                    if let Ok(rec) = serde_json::from_slice::<CachedAtOnly>(v.value()) {
                        all_pages.push((k.value().to_string(), rec.cached_at, v.value().len()));
                    }
                }
            }
        }

        all_pages.sort_by_key(|a| a.1); // Sort oldest first
        let mut pruned = 0;
        let mut cur_bytes = total_bytes;

        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(TABLE_CACHE_PAGES)?;
            for (key, _, size) in all_pages {
                if cur_bytes <= max_bytes as u64 {
                    break;
                }
                let _ = table.remove(key.as_str())?;
                cur_bytes = cur_bytes.saturating_sub(size as u64);
                pruned += 1;
            }
        }
        write_txn.commit()?;
        Ok(pruned)
    }

    pub fn cache_entry_count_and_bytes(&self) -> (usize, u64) {
        let (p_cnt, p_bytes, s_cnt, s_bytes, sum_cnt, sum_bytes) = self.cache_stats_breakdown();
        (p_cnt + s_cnt + sum_cnt, p_bytes + s_bytes + sum_bytes)
    }

    pub fn cache_stats_breakdown(&self) -> (usize, u64, usize, u64, usize, u64) {
        let Ok(read_txn) = self.db.begin_read() else {
            return (0, 0, 0, 0, 0, 0);
        };
        let table_size = |def: TableDefinition<&str, &[u8]>| -> (usize, u64) {
            let mut cnt = 0usize;
            let mut bytes = 0u64;
            if let Ok(table) = read_txn.open_table(def)
                && let Ok(iter) = table.iter()
            {
                for r in iter.flatten() {
                    cnt += 1;
                    bytes += (r.0.value().len() + r.1.value().len()) as u64;
                }
            }
            (cnt, bytes)
        };
        let (pages_cnt, pages_bytes) = table_size(TABLE_CACHE_PAGES);
        let (search_cnt, search_bytes) = table_size(TABLE_CACHE_SEARCH);
        let mut sum_cnt = 0;
        let mut sum_bytes = 0u64;
        if let Ok(ck_table) = read_txn.open_table(TABLE_CACHE_KEY_TO_ID)
            && let Ok(hist_table) = read_txn.open_table(TABLE_HISTORY)
            && let Ok(iter) = ck_table.iter()
        {
            for r in iter.flatten() {
                sum_cnt += 1;
                let id = r.1.value();
                if let Ok(Some(item_val)) = hist_table.get(id)
                    && let Ok(item) = serde_json::from_slice::<LibraryItem>(item_val.value())
                {
                    sum_bytes += item.summary.len() as u64;
                }
            }
        }

        (
            pages_cnt,
            pages_bytes,
            search_cnt,
            search_bytes,
            sum_cnt,
            sum_bytes,
        )
    }

    #[cfg(test)]
    pub(super) fn backdate_page_cache_for_test(&self, url: &str, delta_secs: u64) {
        let Ok(write_txn) = self.db.begin_write() else {
            return;
        };
        if let Ok(mut table) = write_txn.open_table(TABLE_CACHE_PAGES) {
            let updates: Vec<_> = table
                .iter()
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|(k, v)| {
                    let mut rec =
                        serde_json::from_slice::<CachedEntry<crate::scraper::ScrapedPage>>(
                            v.value(),
                        )
                        .ok()?;
                    if rec.value.url != url && k.value() != url {
                        return None;
                    }
                    rec.cached_at = rec.cached_at.saturating_sub(delta_secs);
                    Some((k.value().to_string(), serde_json::to_vec(&rec).ok()?))
                })
                .collect();
            updates.into_iter().for_each(|(k, b)| {
                let _ = table.insert(k.as_str(), b.as_slice());
            });
        }
        let _ = write_txn.commit();
    }

    #[cfg(test)]
    pub(super) fn backdate_history_for_test(&self, title: &str, delta_secs: u64) {
        let Ok(write_txn) = self.db.begin_write() else {
            return;
        };
        if let Ok(mut table) = write_txn.open_table(TABLE_HISTORY) {
            let updates: Vec<_> = table
                .iter()
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|(k, v)| {
                    let mut item = serde_json::from_slice::<LibraryItem>(v.value()).ok()?;
                    if item.title != title && item.url.as_deref() != Some(title) {
                        return None;
                    }
                    let secs = parse_iso_to_unix_secs(&item.created_at)?;
                    item.created_at = format_iso_from_timestamp(secs.saturating_sub(delta_secs));
                    Some((k.value(), serde_json::to_vec(&item).ok()?))
                })
                .collect();
            updates.into_iter().for_each(|(k, b)| {
                let _ = table.insert(k, b.as_slice());
            });
        }
        let _ = write_txn.commit();
    }
}

fn parse_iso_to_unix_secs(s: &str) -> Option<u64> {
    if s.len() < 10 {
        return None;
    }
    let ymd = &s[..10];
    let days = ymd_to_day_number(ymd)?;
    if days < 719468 {
        return None;
    }
    let unix_days = (days - 719468) as u64;
    let mut secs = unix_days * 86400;
    if s.len() >= 16 && (s.as_bytes()[10] == b'T' || s.as_bytes()[10] == b' ') {
        let h: u64 = s[11..13].parse().ok()?;
        let m: u64 = s[14..16].parse().ok()?;
        let s_part: u64 = if s.len() >= 19 {
            s[17..19].parse().unwrap_or(0)
        } else {
            0
        };
        secs += h * 3600 + m * 60 + s_part;
    }
    Some(secs)
}

fn extract_snippet(text: &str, query: &str, terms: &[String]) -> String {
    let match_pos =
        crate::system::text_util::find_ascii_case_insensitive(text, query).or_else(|| {
            terms
                .iter()
                .find_map(|term| crate::system::text_util::find_ascii_case_insensitive(text, term))
        });
    let (start, end) = match match_pos {
        Some(pos) => (
            text.floor_char_boundary(pos.saturating_sub(60)),
            text.ceil_char_boundary((pos + query.len().min(40) + 100).min(text.len())),
        ),
        None => (0, text.ceil_char_boundary(text.len().min(160))),
    };
    let prefix = if start > 0 { "..." } else { "" };
    let suffix = if end < text.len() { "..." } else { "" };
    format!(
        "{prefix}{}{suffix}",
        text[start..end].trim().replace(['\n', '\r'], " ")
    )
}

fn format_date_from_timestamp(secs: u64) -> String {
    let z = (secs / 86400) as i64 + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1024 + doe / 1461 - doe / 142401) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{:04}-{:02}-{:02}", y, m, d)
}

fn ymd_to_day_number(s: &str) -> Option<i64> {
    let mut parts = s.split('-');
    let y: i64 = parts.next()?.parse().ok()?;
    let m: i64 = parts.next()?.parse().ok()?;
    let d: i64 = parts.next()?.parse().ok()?;
    let m = (m + 9) % 12;
    let y = y - m / 10;
    Some(365 * y + y / 4 - y / 100 + y / 400 + (m * 306 + 5) / 10 + (d - 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_summary<'a>(
        target_type: &'a str,
        title: &'a str,
        author: Option<&'a str>,
        url: Option<&'a str>,
        summary: &'a str,
    ) -> NewSummaryRecord<'a> {
        NewSummaryRecord {
            target_type,
            title,
            author,
            url,
            style: "concise",
            model: "gemma-4",
            word_count: 500,
            prompt_tokens: None,
            completion_tokens: None,
            duration_ms: 100,
            tokens_per_second: None,
            summary,
            reasoning: None,
        }
    }

    #[test]
    fn test_database_record_and_history() {
        let db = Database::open_in_memory().unwrap();
        let record = NewSummaryRecord {
            style: "detailed",
            word_count: 11106,
            prompt_tokens: Some(5200),
            completion_tokens: Some(1840),
            duration_ms: 4200,
            tokens_per_second: Some(43.8),
            reasoning: Some("Analyzed Herbert's ecological themes."),
            ..make_summary(
                "book",
                "Dune",
                Some("Frank Herbert"),
                Some("https://en.wikipedia.org/wiki/Dune_(novel)"),
                "Dune is a 1965 epic sci-fi novel about Arrakis, ecology, and Melange spice.",
            )
        };
        db.record_summary(&record).unwrap();

        let history = db.get_history(10).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].title, "Dune");
        assert_eq!(history[0].author.as_deref(), Some("Frank Herbert"));
        assert_eq!(history[0].word_count, 11106);
        assert_eq!(history[0].prompt_tokens, Some(5200));
        assert_eq!(history[0].completion_tokens, Some(1840));

        let items = db.get_all_library_items().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0].reasoning.as_deref(),
            Some("Analyzed Herbert's ecological themes.")
        );
    }

    #[test]
    fn test_database_exports() {
        let db = Database::open_in_memory().unwrap();
        let record = NewSummaryRecord {
            style: "detailed",
            word_count: 11106,
            prompt_tokens: Some(5200),
            completion_tokens: Some(1840),
            duration_ms: 4200,
            tokens_per_second: Some(43.8),
            ..make_summary(
                "book",
                "Dune",
                Some("Frank Herbert"),
                Some("https://en.wikipedia.org/wiki/Dune_(novel)"),
                "Dune is a 1965 epic sci-fi novel about Arrakis.",
            )
        };
        db.record_summary(&record).unwrap();

        let tmp = crate::fs_util::tempdir().unwrap();
        let json_path = tmp.path().join("export.json");
        db.export_json(&json_path).unwrap();
        assert!(
            std::fs::read_to_string(&json_path)
                .unwrap()
                .contains("Dune")
        );

        let vault_dir = tmp.path().join("vault");
        assert_eq!(db.export_markdown_vault(&vault_dir).unwrap(), 1);
        let md_content = std::fs::read_to_string(vault_dir.join("dune.md")).unwrap();
        assert!(md_content.starts_with("---\n"));
        assert!(md_content.contains("author: \"Frank Herbert\""));
        assert!(md_content.contains("# Dune"));
        assert!(md_content.contains("Arrakis"));

        let csv_path = tmp.path().join("export.csv");
        db.export_csv(&csv_path).unwrap();
        let csv_str = std::fs::read_to_string(&csv_path).unwrap();
        assert!(csv_str.contains("Dune"));
        assert!(csv_str.contains("Frank Herbert"));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let csv_meta = std::fs::metadata(&csv_path).unwrap();
            assert_eq!(csv_meta.permissions().mode() & 0o777, 0o600);

            let symlink_target = tmp.path().join("target.csv");
            let symlink_path = tmp.path().join("symlink.csv");
            std::fs::write(&symlink_target, "dummy").unwrap();
            std::os::unix::fs::symlink(&symlink_target, &symlink_path).unwrap();
            let err = db.export_csv(&symlink_path).unwrap_err();
            assert!(err.to_string().contains("symlink"));
        }
    }

    #[test]
    fn test_database_stats() {
        let db = Database::open_in_memory().unwrap();
        db.record_summary(&NewSummaryRecord {
            style: "detailed",
            word_count: 10000,
            prompt_tokens: Some(5000),
            completion_tokens: Some(1500),
            duration_ms: 3000,
            tokens_per_second: Some(50.0),
            ..make_summary(
                "book",
                "Dune",
                Some("Frank Herbert"),
                None,
                "Book summary 1",
            )
        })
        .unwrap();
        db.record_summary(&NewSummaryRecord {
            word_count: 2000,
            prompt_tokens: Some(2500),
            completion_tokens: Some(500),
            duration_ms: 1000,
            tokens_per_second: Some(50.0),
            ..make_summary(
                "web",
                "Rust Article",
                None,
                Some("https://example.com/rust"),
                "Web summary 2",
            )
        })
        .unwrap();

        let stats = db.get_stats().unwrap();
        assert_eq!(stats.total_summaries, 2);
        assert_eq!(stats.book_summaries, 1);
        assert_eq!(stats.web_summaries, 1);
        assert_eq!(stats.file_summaries, 0);
        assert_eq!(stats.stdin_summaries, 0);
        assert_eq!(stats.video_summaries, 0);
        assert_eq!(stats.total_source_words, 12000);
        assert_eq!(stats.total_prompt_tokens, 7500);
        assert_eq!(stats.total_completion_tokens, 2000);
        assert_eq!(stats.total_duration_ms, 4000);
        assert!((stats.avg_tokens_per_second - 50.0).abs() < 0.1);
        assert_eq!(stats.reads_today, 2);
        assert_eq!(stats.active_reading_days, 1);
        assert_eq!(stats.current_streak_days, 1);
        assert!(stats.estimated_minutes_saved > 0.0);
        assert_eq!(stats.models.len(), 1);
        assert_eq!(stats.models[0].model, "gemma-4");
        assert_eq!(stats.models[0].runs, 2);
        assert_eq!(stats.top_domains.len(), 1);
        assert_eq!(stats.top_domains[0].domain, "example.com");
    }

    #[test]
    fn test_database_search_library_and_special_queries() {
        let db = Database::open_in_memory().unwrap();
        db.record_summary(&make_summary(
            "book",
            "Dune",
            Some("Frank Herbert"),
            Some("https://en.wikipedia.org/wiki/Dune_(novel)"),
            "Set on the harsh desert planet Arrakis, the sole source of Melange spice and SQL injection discussions.",
        ))
        .unwrap();

        let hits = db.search("Melange").unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Dune");
        assert!(hits[0].snippet.contains("Melange"));

        for empty_query in ["Cyberpunk", "'; DROP TABLE library; --", "% * _ ?", ""] {
            assert!(db.search(empty_query).unwrap().is_empty(), "{empty_query}");
        }

        assert_eq!(db.search("injection").unwrap().len(), 1);
    }

    #[test]
    fn test_database_search_multibyte_utf8_snippet_safety() {
        let db = Database::open_in_memory().unwrap();
        db.record_summary(&make_summary(
            "book",
            "Le Petit Prince 👑",
            Some("Antoine de Saint-Exupéry"),
            None,
            "🌟 Voici mon secret. Il est très simple: on ne voit bien qu’avec le cœur. L’essentiel est invisible pour les yeux. 🚀🪐✨ 狐狸 and roses.",
        ))
        .unwrap();

        for term in ["secret", "狐狸"] {
            let hits = db.search(term).unwrap();
            assert_eq!(hits.len(), 1);
            assert!(hits[0].snippet.contains(term));
        }
    }

    #[test]
    fn test_database_search_bm25_relevance_ranking() {
        let db = Database::open_in_memory().unwrap();
        db.record_summary(&NewSummaryRecord {
            style: "detailed",
            word_count: 3500,
            ..make_summary(
                "web",
                "Rust Compiler Deep Dive",
                Some("Compiler Team"),
                Some("https://example.com/rust-compiler"),
                "The rust compiler achieves zero-cost abstractions and memory safety through borrow checker analysis. Rust compiler optimizations are extensive.",
            )
        })
        .unwrap();
        db.record_summary(&NewSummaryRecord {
            word_count: 5000,
            ..make_summary(
                "web",
                "General Software Patterns",
                Some("John Doe"),
                Some("https://example.com/patterns"),
                "Design patterns in systems programming. Briefly mentions rust compiler in passing.",
            )
        })
        .unwrap();

        let hits = db.search("Rust Compiler").unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].title, "Rust Compiler Deep Dive");
        assert!(hits[0].score.unwrap() > hits[1].score.unwrap());
        assert!(hits[0].snippet.to_lowercase().contains("rust"));
    }

    #[test]
    fn test_database_library_items_and_deletion() {
        let db = Database::open_in_memory().unwrap();
        db.record_summary(&make_summary(
            "book",
            "Dune",
            Some("Frank Herbert"),
            None,
            "Spice on Arrakis.",
        ))
        .unwrap();
        db.record_summary(&NewSummaryRecord {
            style: "detailed",
            word_count: 800,
            ..make_summary(
                "web",
                "Rust Lang",
                None,
                Some("https://rust-lang.org"),
                "Modern systems programming.",
            )
        })
        .unwrap();

        let items = db.get_all_library_items().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].title, "Rust Lang");
        assert_eq!(items[1].title, "Dune");
        assert_eq!(db.get_latest_summary().unwrap().unwrap().title, "Rust Lang");

        db.delete_library_item(items[0].id).unwrap();
        let items_after = db.get_all_library_items().unwrap();
        assert_eq!(items_after.len(), 1);
        assert_eq!(items_after[0].title, "Dune");
    }

    #[test]
    fn test_database_reset() {
        let db = Database::open_in_memory().unwrap();
        db.record_summary(&make_summary(
            "book",
            "Dune",
            Some("Frank Herbert"),
            None,
            "Dune summary text",
        ))
        .unwrap();
        assert_eq!(db.get_history(10).unwrap().len(), 1);
        assert_eq!(db.get_stats().unwrap().total_summaries, 1);
        assert_eq!(db.search("Dune").unwrap().len(), 1);

        db.reset().unwrap();
        assert_eq!(db.get_history(10).unwrap().len(), 0);
        assert_eq!(db.get_stats().unwrap().total_summaries, 0);
        assert_eq!(db.search("Dune").unwrap().len(), 0);
    }

    #[test]
    fn test_database_file_open_and_reopen() {
        let tmp = crate::fs_util::tempdir().unwrap();
        let db_path = tmp.path().join("reopen.db");

        {
            let db = Database::open_at(&db_path).unwrap();
            db.record_summary(&make_summary(
                "book",
                "Reopen Test",
                None,
                None,
                "Reopen content",
            ))
            .unwrap();
        }

        let db = Database::open_at(&db_path).unwrap();
        let history = db.get_history(10).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].title, "Reopen Test");
        assert_eq!(
            db.get_latest_summary().unwrap().unwrap().summary,
            "Reopen content"
        );
    }

    #[test]
    fn test_library_item_matches_query() {
        let db = Database::open_in_memory().unwrap();
        db.record_summary(&make_summary(
            "book",
            "Foundation and Empire",
            Some("Isaac Asimov"),
            Some("https://example.com/foundation"),
            "The Mule attacks the Galactic Empire.",
        ))
        .unwrap();
        db.record_summary(&make_summary(
            "book",
            "Les Misérables",
            Some("Victor Hugo"),
            None,
            "Jean Valjean and Cosette in 19th-century France.",
        ))
        .unwrap();

        let items = db.get_all_library_items().unwrap();
        let foundation = items
            .iter()
            .find(|i| i.title == "Foundation and Empire")
            .unwrap();
        for q in [
            "foundation",
            "ASIMOV",
            "empire",
            "mule",
            "example.com",
            "book",
            "",
        ] {
            assert!(foundation.matches_query(q), "expected match: {q}");
        }
        for q in ["sandworms", "cyberpunk"] {
            assert!(!foundation.matches_query(q), "expected miss: {q}");
        }

        let les_mis = items.iter().find(|i| i.title == "Les Misérables").unwrap();
        assert!(les_mis.matches_query("Misérables"));
        assert!(les_mis.matches_query("misérables"));
        assert!(!les_mis.matches_query("München"));
    }

    #[test]
    fn test_database_chat_sessions_lifecycle() {
        let db = Database::open_in_memory().unwrap();
        let session_id = db
            .create_chat_session(
                "Rust Concurrency Guide",
                "web",
                Some("https://example.com/rust-concurrency"),
                "gemma-4",
            )
            .unwrap();
        assert!(session_id > 0);

        db.add_chat_turn(
            session_id,
            "What is the difference between Arc and Rc?",
            "Arc is thread-safe (atomic reference counting), whereas Rc is for single-threaded use.",
        )
        .unwrap();
        db.add_chat_turn(
            session_id,
            "When should I use Mutex with Arc?",
            "Wrap the inner value in Mutex or RwLock when multiple threads need mutable access.",
        )
        .unwrap();

        let sessions = db.get_chat_sessions(10).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, session_id);
        assert_eq!(sessions[0].title, "Rust Concurrency Guide");
        assert_eq!(sessions[0].message_count, 4);
        assert_eq!(
            sessions[0].last_question.as_deref(),
            Some("When should I use Mutex with Arc?")
        );

        let (session, messages) = db.get_chat_session(session_id).unwrap().unwrap();
        assert_eq!(session.id, session_id);
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0].role, "user");
        assert_eq!(
            messages[0].content,
            "What is the difference between Arc and Rc?"
        );
        assert_eq!(messages[1].role, "assistant");
        assert_eq!(messages[2].role, "user");
        assert_eq!(messages[3].role, "assistant");

        assert!(db.delete_chat_session(session_id).unwrap());
        assert!(db.get_chat_session(session_id).unwrap().is_none());
        assert!(db.get_chat_sessions(10).unwrap().is_empty());
    }

    #[test]
    #[cfg(unix)]
    fn test_database_refuses_symlink() {
        let tmp = crate::fs_util::tempdir().unwrap();
        let real_db = tmp.path().join("real.db");
        let symlink_db = tmp.path().join("symlink.db");

        {
            let db = Database::open_at(&real_db).expect("Create real db");
            assert!(db.get_stats().is_ok());
        }
        std::os::unix::fs::symlink(&real_db, &symlink_db).unwrap();

        let err = Database::open_at(&symlink_db).err().unwrap();
        assert!(err.to_string().contains("is a symlink"));
    }

    #[test]
    fn test_export_markdown_vault_escaping_and_collisions() {
        let db = Database::open_in_memory().unwrap();
        db.record_summary(&make_summary(
            "web",
            "Article with \"Quotes\" and \nNewlines and \\Backslash",
            Some("Author \"Name\"\nInjectedField: \"true\""),
            Some("https://example.com/page?a=1&b=\"2\"\nmalicious_key: true"),
            "Summary content 1",
        ))
        .unwrap();
        db.record_summary(&NewSummaryRecord {
            word_count: 600,
            duration_ms: 250,
            ..make_summary(
                "web",
                "Article with \"Quotes\" and \nNewlines and \\Backslash",
                Some("Author 2"),
                Some("https://example.com/different"),
                "Summary content 2",
            )
        })
        .unwrap();

        let tmp = crate::fs_util::tempdir().unwrap();
        let vault_dir = tmp.path().join("vault");
        assert_eq!(db.export_markdown_vault(&vault_dir).unwrap(), 2);

        let all_content: Vec<String> = std::fs::read_dir(&vault_dir)
            .unwrap()
            .map(|r| std::fs::read_to_string(r.unwrap().path()).unwrap())
            .collect();
        assert_eq!(all_content.len(), 2);

        for content in &all_content {
            let frontmatter = content.split("---").nth(1).expect("frontmatter block");
            assert!(!frontmatter.contains("\nInjectedField:"));
            assert!(!frontmatter.contains("\nmalicious_key:"));
            assert!(frontmatter.contains("\\\"Quotes\\\""));
            assert!(frontmatter.contains("\\\\Backslash"));
        }
        assert!(
            all_content
                .iter()
                .any(|c| c.contains("\\nmalicious_key: true"))
        );
    }
}
