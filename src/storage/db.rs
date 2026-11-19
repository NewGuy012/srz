use crate::fs_util;
use crate::system::text_util::{slugify, tokenize_search_terms};
use anyhow::Result;
use redb::{Database as RedbDb, ReadableDatabase, ReadableTable, TableDefinition};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const TABLE_HISTORY: TableDefinition<u64, &[u8]> = TableDefinition::new("history");
const TABLE_CACHE_KEY_TO_ID: TableDefinition<&str, u64> = TableDefinition::new("cache_key_to_id");
const TABLE_CHAT_SESSIONS: TableDefinition<u64, &[u8]> = TableDefinition::new("chat_sessions");
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
    #[serde(flatten)]
    pub meta: HistoryEntry,
    pub summary: String,
    #[serde(default)]
    pub reasoning: Option<String>,
}

impl Deref for LibraryItem {
    type Target = HistoryEntry;

    fn deref(&self) -> &Self::Target {
        &self.meta
    }
}

impl DerefMut for LibraryItem {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.meta
    }
}

impl LibraryItem {
    pub fn matches_query(&self, query: &str) -> bool {
        let q = query.trim();
        if q.is_empty() {
            return true;
        }
        let has = |field: &str| crate::system::text_util::contains_case_insensitive(field, q);
        has(&self.title)
            || has(&self.target_type)
            || self.author.as_deref().is_some_and(has)
            || self.url.as_deref().is_some_and(has)
            || has(&self.summary)
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
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    pub created_at: String,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<ChatMessage>,
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
    let parsed = reqwest::Url::parse(url).ok()?;
    matches!(parsed.scheme(), "http" | "https")
        .then(|| parsed.host_str().map(str::to_string))
        .flatten()
}

fn summary_cache_key(target_type: &str, style: &str, model: &str, identifier: &str) -> String {
    format!("{target_type}\0{style}\0{model}\0{}", identifier.trim())
}

fn escape_yaml_string(s: &str) -> String {
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

fn format_csv_cell(cell: &str) -> String {
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

    if safe_cell.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", safe_cell.replace('"', "\"\""))
    } else {
        safe_cell
    }
}

fn prepare_export_file(path: &Path) -> Result<()> {
    fs_util::reject_symlink(path, "export to a symbolic link (CWE-59)")?;
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
        let _ = write_txn.open_table(TABLE_CACHE_PAGES)?;
        let _ = write_txn.open_table(TABLE_CACHE_SEARCH)?;
        let _ = write_txn.open_table(TABLE_CACHE_CAPS)?;
        let _ = write_txn.open_table(TABLE_META)?;
    }
    write_txn.commit()?;
    Ok(())
}

fn delete_cache_tables(write_txn: &redb::WriteTransaction) {
    let _ = write_txn.delete_table(TABLE_CACHE_PAGES);
    let _ = write_txn.delete_table(TABLE_CACHE_SEARCH);
    let _ = write_txn.delete_table(TABLE_CACHE_CAPS);
    let _ = write_txn.delete_table(TABLE_CACHE_KEY_TO_ID);
}

impl Database {
    pub fn default_path() -> PathBuf {
        fs_util::default_data_dir().join("srz.db")
    }

    pub fn open_at(path: &Path) -> Result<Self> {
        if fs_util::reject_symlink(path, "open database symlink").is_err() {
            anyhow::bail!("Refusing to open database: {:?} is a symlink", path);
        }
        if let Some(parent) = path.parent() {
            fs_util::create_private_dir_all(parent)?;
        }

        // Pre-create with 0600 on Unix so new DB files are never briefly 0644.
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let _ = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o600)
                .open(path);
        }

        let mut last_err = None;
        for attempt in 0..5 {
            match RedbDb::create(path) {
                Ok(db) => {
                    let _ = fs_util::set_unix_mode(path, 0o600);
                    init_redb_schema(&db)?;
                    return Ok(Self { db: Arc::new(db) });
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
    fn open_in_memory() -> Result<Self> {
        let db = redb::Builder::new().create_with_backend(redb::backends::InMemoryBackend::new())?;
        init_redb_schema(&db)?;
        Ok(Self { db: Arc::new(db) })
    }

    pub fn reset(&self) -> Result<()> {
        let write_txn = self.db.begin_write()?;
        {
            let _ = write_txn.delete_table(TABLE_HISTORY);
            let _ = write_txn.delete_table(TABLE_CHAT_SESSIONS);
            let _ = write_txn.delete_table(TABLE_META);
            delete_cache_tables(&write_txn);
        }
        write_txn.commit()?;
        init_redb_schema(&self.db)?;
        Ok(())
    }

    pub fn record_summary(
        &self,
        record: &NewSummaryRecord<'_>,
        cache_key: Option<&str>,
    ) -> Result<()> {
        let write_txn = self.db.begin_write()?;
        let id = next_meta_id(&write_txn, "next_history_id")?;

        let item = LibraryItem {
            meta: HistoryEntry {
                id: id as i64,
                target_type: record.target_type.to_string(),
                title: record.title.to_string(),
                author: record.author.map(str::to_string),
                url: record.url.map(str::to_string),
                style: record.style.to_string(),
                model: record.model.to_string(),
                word_count: record.word_count as u32,
                prompt_tokens: record.prompt_tokens,
                completion_tokens: record.completion_tokens,
                duration_ms: record.duration_ms,
                tokens_per_second: record.tokens_per_second,
                created_at: chrono_now_iso(),
            },
            summary: record.summary.to_string(),
            reasoning: record.reasoning.map(str::to_string),
        };

        let bytes = serde_json::to_vec(&item)?;
        {
            let mut hist_table = write_txn.open_table(TABLE_HISTORY)?;
            hist_table.insert(id, bytes.as_slice())?;
        }

        if let Some(ck) = cache_key {
            let composite = summary_cache_key(record.target_type, record.style, record.model, ck);
            let mut ck_table = write_txn.open_table(TABLE_CACHE_KEY_TO_ID)?;
            ck_table.insert(composite.as_str(), id)?;
        }

        write_txn.commit()?;
        Ok(())
    }

    pub fn get_history(&self, limit: usize) -> Result<Vec<HistoryEntry>> {
        let items = self.get_library_items_bounded(limit)?;
        Ok(items.into_iter().map(|item| item.meta).collect())
    }

    pub fn get_all_library_items(&self) -> Result<Vec<LibraryItem>> {
        self.get_library_items_bounded(usize::MAX)
    }

    pub fn get_library_items_bounded(&self, limit: usize) -> Result<Vec<LibraryItem>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let read_txn = self.db.begin_read()?;
        let Ok(table) = read_txn.open_table(TABLE_HISTORY) else {
            return Ok(Vec::new());
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
        if id <= 0 {
            return Ok(None);
        }
        let read_txn = self.db.begin_read()?;
        let Ok(table) = read_txn.open_table(TABLE_HISTORY) else {
            return Ok(None);
        };

        if let Some(val) = table.get(id as u64)? {
            return Ok(Some(serde_json::from_slice(val.value())?));
        }
        Ok(None)
    }

    pub fn get_latest_summary(&self) -> Result<Option<LibraryItem>> {
        let read_txn = self.db.begin_read()?;
        let Ok(table) = read_txn.open_table(TABLE_HISTORY) else {
            return Ok(None);
        };

        if let Some(last_res) = table.iter()?.next_back() {
            let (_, val) = last_res?;
            return Ok(Some(serde_json::from_slice(val.value())?));
        }
        Ok(None)
    }

    pub fn delete_library_item(&self, id: i64) -> Result<()> {
        if id <= 0 {
            return Ok(());
        }
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
            target_url: target_url.map(str::to_string),
            model: model.to_string(),
            message_count: 0,
            last_question: None,
            created_at: now.clone(),
            updated_at: now,
            messages: Vec::new(),
        };

        let bytes = serde_json::to_vec(&session)?;
        {
            let mut sess_table = write_txn.open_table(TABLE_CHAT_SESSIONS)?;
            sess_table.insert(id, bytes.as_slice())?;
        }

        write_txn.commit()?;
        Ok(id as i64)
    }

    pub fn add_chat_turn(&self, session_id: i64, question: &str, answer: &str) -> Result<()> {
        if session_id <= 0 {
            anyhow::bail!("Invalid chat session ID: {}", session_id);
        }
        let sess_u64 = session_id as u64;
        let write_txn = self.db.begin_write()?;
        let now = chrono_now_iso();

        {
            let mut sess_table = write_txn.open_table(TABLE_CHAT_SESSIONS)?;
            let val = sess_table
                .get(sess_u64)?
                .ok_or_else(|| anyhow::anyhow!("Chat session #{} not found", session_id))?;
            let mut session: ChatSession = serde_json::from_slice(val.value())?;
            drop(val);

            session.messages.push(ChatMessage {
                role: "user".to_string(),
                content: question.to_string(),
                created_at: now.clone(),
            });
            session.messages.push(ChatMessage {
                role: "assistant".to_string(),
                content: answer.to_string(),
                created_at: now.clone(),
            });
            session.message_count += 2;
            session.last_question = Some(question.to_string());
            session.updated_at = now;

            let bytes = serde_json::to_vec(&session)?;
            sess_table.insert(sess_u64, bytes.as_slice())?;
        }

        write_txn.commit()?;
        Ok(())
    }

    pub fn get_chat_sessions(&self, limit: usize) -> Result<Vec<ChatSession>> {
        let read_txn = self.db.begin_read()?;
        let Ok(table) = read_txn.open_table(TABLE_CHAT_SESSIONS) else {
            return Ok(Vec::new());
        };

        let mut sessions = Vec::new();
        for item_res in table.iter()? {
            let (_, val) = item_res?;
            let mut session: ChatSession = serde_json::from_slice(val.value())?;
            session.messages.clear();
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
        if session_id <= 0 {
            return Ok(None);
        }
        let read_txn = self.db.begin_read()?;
        let Ok(sess_table) = read_txn.open_table(TABLE_CHAT_SESSIONS) else {
            return Ok(None);
        };
        let Some(val) = sess_table.get(session_id as u64)? else {
            return Ok(None);
        };
        let mut session: ChatSession = serde_json::from_slice(val.value())?;
        let messages = std::mem::take(&mut session.messages);
        Ok(Some((session, messages)))
    }

    pub fn delete_chat_session(&self, session_id: i64) -> Result<bool> {
        if session_id <= 0 {
            return Ok(false);
        }
        let write_txn = self.db.begin_write()?;
        let removed = {
            let mut sess_table = write_txn.open_table(TABLE_CHAT_SESSIONS)?;
            sess_table.remove(session_id as u64)?.is_some()
        };
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

            let item_ymd = item.created_at.get(..10).unwrap_or("");
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

        for item in items {
            let mut title_matches = 0usize;
            let mut body_matches = 0usize;

            for term in &query_terms {
                if crate::system::text_util::contains_case_insensitive(&item.title, term)
                    || item.author.as_deref().is_some_and(|a| {
                        crate::system::text_util::contains_case_insensitive(a, term)
                    })
                {
                    title_matches += 1;
                }
                if crate::system::text_util::contains_case_insensitive(&item.summary, term)
                    || item.url.as_deref().is_some_and(|u| {
                        crate::system::text_util::contains_case_insensitive(u, term)
                    })
                {
                    body_matches += 1;
                }
            }

            if title_matches > 0 || body_matches > 0 {
                let score = (title_matches as f64 * 10.0) + (body_matches as f64);
                let snippet = extract_snippet(&item.summary, trimmed, &query_terms);

                results.push(LibrarySearchResult {
                    id: item.id,
                    target_type: item.target_type.clone(),
                    title: item.title.clone(),
                    author: item.author.clone(),
                    url: item.url.clone(),
                    snippet,
                    created_at: item.created_at.clone(),
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
        let mut csv = String::from(
            "id,target_type,title,author,url,style,model,word_count,prompt_tokens,completion_tokens,duration_ms,tokens_per_second,created_at,summary\n",
        );
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
        let ck_table = read_txn.open_table(TABLE_CACHE_KEY_TO_ID).ok()?;
        let hist_table = read_txn.open_table(TABLE_HISTORY).ok()?;
        let composite = summary_cache_key(target_type, style, model, identifier);
        let id_val = ck_table
            .get(composite.as_str())
            .ok()
            .flatten()
            .or_else(|| ck_table.get(identifier.trim()).ok().flatten())?;
        let val = hist_table.get(id_val.value()).ok()??;
        let item = serde_json::from_slice::<LibraryItem>(val.value()).ok()?;
        let created_secs = parse_iso_to_unix_secs(&item.created_at)?;

        if item.target_type == target_type
            && item.style == style
            && item.model == model
            && now_unix_secs().saturating_sub(created_secs) <= ttl.as_secs()
        {
            Some(crate::storage::cache::CachedSummary {
                title: item.meta.title,
                author: item.meta.author,
                summary: item.summary,
                word_count: item.meta.word_count as usize,
                reasoning: item.reasoning,
            })
        } else {
            None
        }
    }

    pub fn clear_cache_tables(&self) -> Result<()> {
        let write_txn = self.db.begin_write()?;
        delete_cache_tables(&write_txn);
        write_txn.commit()?;
        init_redb_schema(&self.db)?;
        Ok(())
    }

    pub fn prune_cache_by_age(&self, max_age: Duration) -> Result<(usize, u64)> {
        let now = now_unix_secs();
        let max_age_secs = max_age.as_secs();
        let mut pruned = 0;
        let mut reclaimed = 0u64;

        let write_txn = self.db.begin_write()?;
        for table_def in [TABLE_CACHE_PAGES, TABLE_CACHE_SEARCH, TABLE_CACHE_CAPS] {
            let mut table = write_txn.open_table(table_def)?;
            let mut to_delete = Vec::new();
            for item_res in table.iter()? {
                let (k, v) = item_res?;
                if let Ok(rec) = serde_json::from_slice::<CachedAtOnly>(v.value())
                    && now.saturating_sub(rec.cached_at) > max_age_secs
                {
                    let bytes = (k.value().len() + v.value().len()) as u64;
                    to_delete.push((k.value().to_string(), bytes));
                }
            }
            for (k, bytes) in to_delete {
                let _ = table.remove(k.as_str())?;
                pruned += 1;
                reclaimed = reclaimed.saturating_add(bytes);
            }
        }
        write_txn.commit()?;
        Ok((pruned, reclaimed))
    }

    pub fn prune_cache_lru(&self, max_bytes: usize) -> Result<(usize, u64)> {
        let mut entries: Vec<(bool, String, u64, u64)> = Vec::new();
        let mut total_bytes = 0u64;
        {
            let read_txn = self.db.begin_read()?;
            for (is_pages, table_def) in [(true, TABLE_CACHE_PAGES), (false, TABLE_CACHE_SEARCH)] {
                if let Ok(table) = read_txn.open_table(table_def) {
                    for item_res in table.iter()? {
                        let (k, v) = item_res?;
                        let size = (k.value().len() + v.value().len()) as u64;
                        total_bytes = total_bytes.saturating_add(size);
                        let cached_at = serde_json::from_slice::<CachedAtOnly>(v.value())
                            .map(|r| r.cached_at)
                            .unwrap_or(0);
                        entries.push((is_pages, k.value().to_string(), cached_at, size));
                    }
                }
            }
        }

        if total_bytes <= max_bytes as u64 || entries.is_empty() {
            return Ok((0, 0));
        }

        entries.sort_by_key(|e| e.2);
        let mut pruned = 0;
        let mut reclaimed = 0u64;
        let mut cur_bytes = total_bytes;

        let write_txn = self.db.begin_write()?;
        {
            let mut pages = write_txn.open_table(TABLE_CACHE_PAGES)?;
            let mut search = write_txn.open_table(TABLE_CACHE_SEARCH)?;
            for (is_pages, key, _, size) in entries {
                if cur_bytes <= max_bytes as u64 {
                    break;
                }
                let table = if is_pages { &mut pages } else { &mut search };
                let _ = table.remove(key.as_str())?;
                cur_bytes = cur_bytes.saturating_sub(size);
                reclaimed = reclaimed.saturating_add(size);
                pruned += 1;
            }
        }
        write_txn.commit()?;
        Ok((pruned, reclaimed))
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
                if let Ok(Some(item_val)) = hist_table.get(r.1.value())
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
            for (k, b) in updates {
                let _ = table.insert(k.as_str(), b.as_slice());
            }
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
            for (k, b) in updates {
                let _ = table.insert(k, b.as_slice());
            }
        }
        let _ = write_txn.commit();
    }
}

fn parse_iso_to_unix_secs(s: &str) -> Option<u64> {
    if !s.is_ascii() || s.len() < 10 {
        return None;
    }
    let days = ymd_to_day_number(&s[..10])?;
    if days < 719468 {
        return None;
    }
    let unix_days = (days - 719468) as u64;
    let mut secs = unix_days * 86400;
    if s.len() >= 16 && (s.as_bytes()[10] == b'T' || s.as_bytes()[10] == b' ') {
        let h: u64 = s[11..13].parse().ok()?;
        let m: u64 = s[14..16].parse().ok()?;
        let s_part: u64 = s.get(17..19).and_then(|v| v.parse().ok()).unwrap_or(0);
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
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
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
    if parts.next().is_some()
        || !(1970..=9999).contains(&y)
        || !(1..=12).contains(&m)
        || !(1..=31).contains(&d)
    {
        return None;
    }
    let m = (m + 9) % 12;
    let y = y - m / 10;
    Some(365 * y + y / 4 - y / 100 + y / 400 + (m * 306 + 5) / 10 + (d - 1))
}

#[cfg(test)]
mod tests {
    use super::super::make_summary;
    use super::*;

    #[test]
    fn test_calendar_date_conversion_no_underflow() {
        for ymd in [
            "1970-01-01",
            "2000-02-29",
            "2024-02-29",
            "2026-02-26",
            "2026-02-28",
            "2026-03-01",
            "2026-12-31",
            "2100-02-28",
        ] {
            let days = ymd_to_day_number(ymd).expect("valid ymd");
            let unix_secs = ((days - 719468) as u64) * 86400;
            assert_eq!(format_date_from_timestamp(unix_secs), ymd);
        }
        assert!(ymd_to_day_number("99999999999999999-01-01").is_none());
        assert!(parse_iso_to_unix_secs("2026年02月28").is_none());
    }

    #[test]
    fn test_format_csv_cell_blocks_formula_bypasses() {
        for dangerous in [
            "\t=cmd|'/C calc'!A0",
            "\r=1+1",
            "\t123",
            "  =SUM(A1:A2)",
            " +cmd",
            "+inf",
            "-inf",
            "+NaN",
            "-nan",
            "--42",
            "+-42",
        ] {
            let formatted = format_csv_cell(dangerous);
            let unquoted = formatted.strip_prefix('"').unwrap_or(&formatted);
            assert!(unquoted.starts_with('\''), "payload: {dangerous:?}");
        }
        assert_eq!(format_csv_cell("-42.5"), "-42.5");
        assert_eq!(format_csv_cell("+100"), "+100");
        assert_eq!(format_csv_cell("normal text"), "normal text");
        assert_eq!(format_csv_cell("hello, world"), "\"hello, world\"");
    }

    #[test]
    fn test_summary_cache_key_disambiguates_styles_and_models() {
        let db = Database::open_in_memory().unwrap();
        let url = "https://example.com/article";
        db.record_summary(
            &NewSummaryRecord {
                style: "concise",
                model: "m1",
                ..make_summary("web", "Title", None, Some(url), "Concise summary")
            },
            Some(url),
        )
        .unwrap();
        db.record_summary(
            &NewSummaryRecord {
                style: "detailed",
                model: "m1",
                ..make_summary("web", "Title", None, Some(url), "Detailed summary")
            },
            Some(url),
        )
        .unwrap();

        let ttl = Duration::from_secs(3600);
        assert_eq!(
            db.get_cached_summary("web", url, "concise", "m1", ttl)
                .unwrap()
                .summary,
            "Concise summary"
        );
        assert_eq!(
            db.get_cached_summary("web", url, "detailed", "m1", ttl)
                .unwrap()
                .summary,
            "Detailed summary"
        );
    }

    #[test]
    fn test_database_crud_history_and_reset() {
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
                "Dune is a 1965 epic sci-fi novel about Arrakis.",
            )
        };
        db.record_summary(&record, None).unwrap();
        db.record_summary(
            &NewSummaryRecord {
                style: "detailed",
                word_count: 800,
                ..make_summary(
                    "web",
                    "Rust Lang",
                    None,
                    Some("https://rust-lang.org"),
                    "Modern systems programming.",
                )
            },
            None,
        )
        .unwrap();

        let history = db.get_history(10).unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].title, "Rust Lang");
        assert_eq!(history[1].title, "Dune");
        assert_eq!(history[1].author.as_deref(), Some("Frank Herbert"));
        assert_eq!(db.get_latest_summary().unwrap().unwrap().title, "Rust Lang");

        let items = db.get_all_library_items().unwrap();
        assert_eq!(
            items[1].reasoning.as_deref(),
            Some("Analyzed Herbert's ecological themes.")
        );

        db.delete_library_item(items[0].id).unwrap();
        assert_eq!(db.get_all_library_items().unwrap().len(), 1);

        db.reset().unwrap();
        assert!(db.get_history(10).unwrap().is_empty());
        assert_eq!(db.get_stats().unwrap().total_summaries, 0);
    }

    #[test]
    fn test_database_exports_and_vault_security() {
        let db = Database::open_in_memory().unwrap();
        db.record_summary(
            &make_summary(
                "web",
                "Article with \"Quotes\" and \nNewlines and \\Backslash",
                Some("Author \"Name\"\nInjectedField: \"true\""),
                Some("https://example.com/page?a=1&b=\"2\"\nmalicious_key: true"),
                "Summary content 1",
            ),
            None,
        )
        .unwrap();
        db.record_summary(
            &make_summary(
                "web",
                "Article with \"Quotes\" and \nNewlines and \\Backslash",
                Some("Author 2"),
                Some("https://example.com/different"),
                "Summary content 2",
            ),
            None,
        )
        .unwrap();

        let tmp = crate::fs_util::tempdir().unwrap();
        let json_path = tmp.path().join("export.json");
        db.export_json(&json_path).unwrap();
        assert!(
            std::fs::read_to_string(&json_path)
                .unwrap()
                .contains("Summary content 1")
        );

        let csv_path = tmp.path().join("export.csv");
        db.export_csv(&csv_path).unwrap();
        let csv_str = std::fs::read_to_string(&csv_path).unwrap();
        assert!(csv_str.contains("Summary content 2"));

        let vault_dir = tmp.path().join("vault");
        assert_eq!(db.export_markdown_vault(&vault_dir).unwrap(), 2);
        let all_content: Vec<String> = std::fs::read_dir(&vault_dir)
            .unwrap()
            .map(|r| std::fs::read_to_string(r.unwrap().path()).unwrap())
            .collect();
        for content in &all_content {
            let frontmatter = content.split("---").nth(1).expect("frontmatter block");
            assert!(!frontmatter.contains("\nInjectedField:"));
            assert!(!frontmatter.contains("\nmalicious_key:"));
            assert!(frontmatter.contains("\\\"Quotes\\\""));
            assert!(frontmatter.contains("\\\\Backslash"));
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&csv_path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            let symlink_target = tmp.path().join("target.csv");
            let symlink_path = tmp.path().join("symlink.csv");
            std::fs::write(&symlink_target, "dummy").unwrap();
            std::os::unix::fs::symlink(&symlink_target, &symlink_path).unwrap();
            assert!(
                db.export_csv(&symlink_path)
                    .unwrap_err()
                    .to_string()
                    .contains("symlink")
            );
        }
    }

    #[test]
    fn test_database_stats() {
        let db = Database::open_in_memory().unwrap();
        db.record_summary(
            &NewSummaryRecord {
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
            },
            None,
        )
        .unwrap();
        db.record_summary(
            &NewSummaryRecord {
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
            },
            None,
        )
        .unwrap();

        let stats = db.get_stats().unwrap();
        assert_eq!(stats.total_summaries, 2);
        assert_eq!((stats.book_summaries, stats.web_summaries), (1, 1));
        assert_eq!(stats.total_source_words, 12000);
        assert_eq!(stats.total_prompt_tokens, 7500);
        assert_eq!(stats.total_completion_tokens, 2000);
        assert_eq!(stats.reads_today, 2);
        assert_eq!(stats.current_streak_days, 1);
        assert_eq!(stats.models[0].runs, 2);
        assert_eq!(stats.top_domains[0].domain, "example.com");
    }

    #[test]
    fn test_database_search_ranking_and_utf8_safety() {
        let db = Database::open_in_memory().unwrap();
        db.record_summary(
            &make_summary(
                "web",
                "Rust Compiler Deep Dive",
                Some("Compiler Team"),
                Some("https://example.com/rust-compiler"),
                "The rust compiler achieves zero-cost abstractions and memory safety.",
            ),
            None,
        )
        .unwrap();
        db.record_summary(
            &make_summary(
                "web",
                "General Software Patterns",
                Some("John Doe"),
                Some("https://example.com/patterns"),
                "Design patterns in systems programming. Mentions rust compiler and 狐狸 🌟.",
            ),
            None,
        )
        .unwrap();

        let hits = db.search("Rust Compiler").unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].title, "Rust Compiler Deep Dive");
        assert!(hits[0].score.unwrap() > hits[1].score.unwrap());

        // Author match works even when author isn't repeated in summary
        assert_eq!(db.search("John Doe").unwrap().len(), 1);

        // Multi-byte UTF-8 snippet slicing and special queries
        assert!(db.search("狐狸").unwrap()[0].snippet.contains("狐狸"));
        for empty_query in ["Cyberpunk", "'; DROP TABLE library; --", "% * _ ?", ""] {
            assert!(db.search(empty_query).unwrap().is_empty());
        }
    }

    #[test]
    fn test_library_item_matches_query() {
        let db = Database::open_in_memory().unwrap();
        db.record_summary(
            &make_summary(
                "book",
                "Foundation and Empire",
                Some("Isaac Asimov"),
                Some("https://example.com/foundation"),
                "The Mule attacks the Galactic Empire.",
            ),
            None,
        )
        .unwrap();
        db.record_summary(
            &make_summary(
                "book",
                "Les Misérables",
                Some("Victor Hugo"),
                None,
                "Jean Valjean in France.",
            ),
            None,
        )
        .unwrap();

        let items = db.get_all_library_items().unwrap();
        let foundation = items
            .iter()
            .find(|i| i.title == "Foundation and Empire")
            .unwrap();
        for q in ["foundation", "ASIMOV", "mule", "example.com", "book", ""] {
            assert!(foundation.matches_query(q), "{q}");
        }
        assert!(!foundation.matches_query("sandworms"));

        let les_mis = items.iter().find(|i| i.title == "Les Misérables").unwrap();
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
        assert!(db.add_chat_turn(-1, "q", "a").is_err());

        db.add_chat_turn(
            session_id,
            "Arc vs Rc?",
            "Arc is atomic; Rc is single-threaded.",
        )
        .unwrap();
        db.add_chat_turn(
            session_id,
            "Use Mutex?",
            "Wrap shared mutable state in Mutex.",
        )
        .unwrap();

        let sessions = db.get_chat_sessions(10).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].message_count, 4);
        assert!(sessions[0].messages.is_empty());
        assert_eq!(sessions[0].last_question.as_deref(), Some("Use Mutex?"));

        let (session, messages) = db.get_chat_session(session_id).unwrap().unwrap();
        assert_eq!(session.id, session_id);
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[1].role, "assistant");

        assert!(db.delete_chat_session(session_id).unwrap());
        assert!(db.get_chat_session(session_id).unwrap().is_none());
        assert!(db.get_chat_sessions(10).unwrap().is_empty());
    }

    #[test]
    fn test_database_file_open_reopen_and_symlink_rejection() {
        let tmp = crate::fs_util::tempdir().unwrap();
        let db_path = tmp.path().join("reopen.db");
        {
            let db = Database::open_at(&db_path).unwrap();
            db.record_summary(
                &make_summary("book", "Reopen Test", None, None, "Reopen content"),
                None,
            )
            .unwrap();
        }
        let db = Database::open_at(&db_path).unwrap();
        assert_eq!(db.get_history(10).unwrap().len(), 1);

        #[cfg(unix)]
        {
            let symlink_db = tmp.path().join("symlink.db");
            std::os::unix::fs::symlink(&db_path, &symlink_db).unwrap();
            assert!(
                Database::open_at(&symlink_db)
                    .err()
                    .unwrap()
                    .to_string()
                    .contains("is a symlink")
            );
        }
    }
}
