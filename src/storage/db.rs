use crate::fs_util;
use crate::system::text_util::{slugify, tokenize_search_terms};
use anyhow::Result;
use redb::{Database as RedbDb, ReadableDatabase, ReadableTable, TableDefinition};
use serde::de::{DeserializeOwned, IgnoredAny};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const TABLE_HISTORY: TableDefinition<u64, &[u8]> = TableDefinition::new("history");
const TABLE_CACHE_KEY_TO_ID: TableDefinition<&str, u64> = TableDefinition::new("cache_key_to_id");
const TABLE_CHAT_SESSIONS: TableDefinition<u64, &[u8]> = TableDefinition::new("chat_sessions");
const TABLE_CACHE_PAGES: TableDefinition<&str, &[u8]> = TableDefinition::new("cache_pages");
const TABLE_CACHE_SEARCH: TableDefinition<&str, &[u8]> = TableDefinition::new("cache_search");
const TABLE_CACHE_CAPS: TableDefinition<&str, &[u8]> = TableDefinition::new("cache_caps");
const TABLE_META: TableDefinition<&str, u64> = TableDefinition::new("meta");

const UNIX_EPOCH_DAYS: i64 = 719_468;

#[derive(Clone, Copy)]
pub(super) enum CacheTable {
    Pages,
    Search,
    Caps,
}

impl CacheTable {
    const fn def(self) -> TableDefinition<'static, &'static str, &'static [u8]> {
        match self {
            Self::Pages => TABLE_CACHE_PAGES,
            Self::Search => TABLE_CACHE_SEARCH,
            Self::Caps => TABLE_CACHE_CAPS,
        }
    }
}

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
            '\u{0085}' => out.push_str("\\u0085"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c if (c as u32) < 0x20 || c == '\x7f' => {
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

fn init_schema_in_txn(write_txn: &redb::WriteTransaction) -> Result<()> {
    let _ = write_txn.open_table(TABLE_HISTORY)?;
    let _ = write_txn.open_table(TABLE_CACHE_KEY_TO_ID)?;
    let _ = write_txn.open_table(TABLE_CHAT_SESSIONS)?;
    let _ = write_txn.open_table(TABLE_CACHE_PAGES)?;
    let _ = write_txn.open_table(TABLE_CACHE_SEARCH)?;
    let _ = write_txn.open_table(TABLE_CACHE_CAPS)?;
    let mut meta = write_txn.open_table(TABLE_META)?;
    if meta.get("schema_version")?.is_none() {
        meta.insert("schema_version", 1u64)?;
    }
    Ok(())
}

fn delete_cache_tables(write_txn: &redb::WriteTransaction) {
    let _ = write_txn.delete_table(TABLE_CACHE_PAGES);
    let _ = write_txn.delete_table(TABLE_CACHE_SEARCH);
    let _ = write_txn.delete_table(TABLE_CACHE_CAPS);
    let _ = write_txn.delete_table(TABLE_CACHE_KEY_TO_ID);
}

fn prune_table_older_than(
    write_txn: &redb::WriteTransaction,
    table_def: TableDefinition<&str, &[u8]>,
    max_age_secs: u64,
    now: u64,
) -> Result<(usize, u64)> {
    let mut table = write_txn.open_table(table_def)?;
    let mut to_delete = Vec::new();
    for item_res in table.iter()? {
        let (k, v) = item_res?;
        if let Ok(rec) = serde_json::from_slice::<CachedEntry<IgnoredAny>>(v.value())
            && now.saturating_sub(rec.cached_at) > max_age_secs
        {
            to_delete.push((
                k.value().to_string(),
                (k.value().len() + v.value().len()) as u64,
            ));
        }
    }
    let mut pruned = 0;
    let mut reclaimed = 0u64;
    for (k, bytes) in to_delete {
        let _ = table.remove(k.as_str())?;
        pruned += 1;
        reclaimed = reclaimed.saturating_add(bytes);
    }
    Ok((pruned, reclaimed))
}

impl Database {
    pub fn default_path() -> PathBuf {
        fs_util::default_data_dir().join("srz.db")
    }

    pub fn open_at(path: &Path) -> Result<Self> {
        if fs_util::reject_symlink(path, "open database symlink").is_err() {
            anyhow::bail!("Refusing to open database: {:?} is a symlink", path);
        }
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
            && !parent.exists()
        {
            fs_util::create_private_dir_all(parent)?;
        }

        // Pre-create new DB files atomically with 0600 on Unix.
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let _ = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(path);
        }

        let mut last_err = None;
        for attempt in 0..5 {
            match RedbDb::create(path) {
                Ok(db) => {
                    let _ = fs_util::set_unix_mode(path, 0o600);
                    let write_txn = db.begin_write()?;
                    init_schema_in_txn(&write_txn)?;
                    write_txn.commit()?;
                    return Ok(Self { db: Arc::new(db) });
                }
                Err(err) => {
                    let retryable = matches!(err, redb::DatabaseError::DatabaseAlreadyOpen);
                    last_err = Some(err);
                    if !retryable || attempt == 4 {
                        break;
                    }
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
        let write_txn = db.begin_write()?;
        init_schema_in_txn(&write_txn)?;
        write_txn.commit()?;
        Ok(Self { db: Arc::new(db) })
    }

    pub fn reset(&self) -> Result<()> {
        let write_txn = self.db.begin_write()?;
        let _ = write_txn.delete_table(TABLE_HISTORY);
        let _ = write_txn.delete_table(TABLE_CHAT_SESSIONS);
        let _ = write_txn.delete_table(TABLE_META);
        delete_cache_tables(&write_txn);
        init_schema_in_txn(&write_txn)?;
        write_txn.commit()?;
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
                word_count: u32::try_from(record.word_count).unwrap_or(u32::MAX),
                prompt_tokens: record.prompt_tokens,
                completion_tokens: record.completion_tokens,
                duration_ms: record.duration_ms,
                tokens_per_second: record.tokens_per_second,
                created_at: format_iso_from_timestamp(now_unix_secs()),
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

    fn get_library_items_bounded(&self, limit: usize) -> Result<Vec<LibraryItem>> {
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
            let Ok(item) = serde_json::from_slice::<LibraryItem>(val.value()) else {
                continue;
            };
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
        Ok(table
            .get(id as u64)?
            .and_then(|val| serde_json::from_slice(val.value()).ok()))
    }

    pub fn get_latest_summary(&self) -> Result<Option<LibraryItem>> {
        Ok(self.get_library_items_bounded(1)?.into_iter().next())
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

        let now = format_iso_from_timestamp(now_unix_secs());
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
        let now = format_iso_from_timestamp(now_unix_secs());

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
            let Ok(mut session) = serde_json::from_slice::<ChatSession>(val.value()) else {
                continue;
            };
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
        let mut active_day_nums: HashSet<i64> = HashSet::new();

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

            stats.total_source_words = stats
                .total_source_words
                .saturating_add(item.word_count as i64);
            stats.total_summary_words = stats
                .total_summary_words
                .saturating_add(item.summary.split_ascii_whitespace().count() as i64);
            stats.total_prompt_tokens = stats
                .total_prompt_tokens
                .saturating_add(item.prompt_tokens.unwrap_or(0) as i64);
            stats.total_completion_tokens = stats
                .total_completion_tokens
                .saturating_add(item.completion_tokens.unwrap_or(0) as i64);
            stats.total_duration_ms = stats
                .total_duration_ms
                .saturating_add(item.duration_ms.min(i64::MAX as u64) as i64);

            let valid_tps = item.tokens_per_second.filter(|&t| t.is_finite() && t > 0.0);
            if let Some(tps) = valid_tps {
                total_tps += tps;
                tps_count += 1;
                if tps > stats.max_tokens_per_second {
                    stats.max_tokens_per_second = tps;
                }
            }

            if let Some(item_ymd) = item.created_at.get(..10) {
                if let Some(day_num) = ymd_to_day_number(item_ymd) {
                    active_day_nums.insert(day_num);
                }
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
            entry.1 = entry
                .1
                .saturating_add(item.completion_tokens.unwrap_or(0) as i64);
            if let Some(tps) = valid_tps {
                entry.2 += tps;
                entry.4.push(tps);
            }
            entry.3 = entry
                .3
                .saturating_add(item.duration_ms.min(i64::MAX as u64) as i64);

            if let Some(ref u) = item.url
                && let Some(domain) = extract_domain(u)
            {
                let d_entry = domain_map.entry(domain).or_insert((0, 0));
                d_entry.0 += 1;
                d_entry.1 = d_entry.1.saturating_add(item.word_count as i64);
            }
        }

        if tps_count > 0 {
            stats.avg_tokens_per_second = total_tps / tps_count as f64;
        }
        stats.active_reading_days = active_day_nums.len() as i64;
        if stats.total_source_words > stats.total_summary_words {
            stats.estimated_minutes_saved =
                (stats.total_source_words - stats.total_summary_words) as f64 / 225.0;
        }

        let mut day_nums: Vec<i64> = active_day_nums.into_iter().collect();
        day_nums.sort_by(|a, b| b.cmp(a));
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
        let items = self.get_all_library_items()?;
        let mut csv = String::from(
            "id,target_type,title,author,url,style,model,word_count,prompt_tokens,completion_tokens,duration_ms,tokens_per_second,created_at,summary\n",
        );
        for item in items {
            let _ = write!(
                csv,
                "{},{},{},{},{},{},{},{},",
                item.id,
                format_csv_cell(&item.target_type),
                format_csv_cell(&item.title),
                format_csv_cell(item.author.as_deref().unwrap_or("")),
                format_csv_cell(item.url.as_deref().unwrap_or("")),
                format_csv_cell(&item.style),
                format_csv_cell(&item.model),
                item.word_count,
            );
            if let Some(pt) = item.prompt_tokens {
                let _ = write!(csv, "{}", pt);
            }
            csv.push(',');
            if let Some(ct) = item.completion_tokens {
                let _ = write!(csv, "{}", ct);
            }
            let _ = write!(csv, ",{},", item.duration_ms);
            if let Some(tps) = item.tokens_per_second {
                let _ = write!(csv, "{:.1}", tps);
            }
            let _ = writeln!(
                csv,
                ",{},{}",
                format_csv_cell(&item.created_at),
                format_csv_cell(&item.summary),
            );
        }
        fs_util::write_private_file(path, csv.as_bytes())?;
        Ok(())
    }

    pub fn export_json(&self, path: &Path) -> Result<()> {
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
            let _ = writeln!(content, "---");
            let _ = writeln!(content, "title: \"{}\"", escape_yaml_string(&item.title));
            let _ = writeln!(
                content,
                "target_type: \"{}\"",
                escape_yaml_string(&item.target_type)
            );
            if let Some(author) = &item.author {
                let _ = writeln!(content, "author: \"{}\"", escape_yaml_string(author));
            }
            if let Some(url) = &item.url {
                let _ = writeln!(content, "url: \"{}\"", escape_yaml_string(url));
            }
            let _ = writeln!(content, "model: \"{}\"", escape_yaml_string(&item.model));
            let _ = writeln!(content, "style: \"{}\"", escape_yaml_string(&item.style));
            let _ = writeln!(content, "word_count: {}", item.word_count);
            let _ = writeln!(
                content,
                "created_at: \"{}\"",
                escape_yaml_string(&item.created_at)
            );
            let clean_title = item.title.replace(['\n', '\r'], " ");
            let _ = writeln!(content, "---\n\n# {}\n\n{}", clean_title, item.summary);

            fs_util::write_private_file(&file_path, content.as_bytes())?;
            written += 1;
        }

        Ok(written)
    }

    pub(super) fn get_cached_json<T: DeserializeOwned>(
        &self,
        table_kind: CacheTable,
        key: &str,
        ttl: Duration,
    ) -> Option<T> {
        let read_txn = self.db.begin_read().ok()?;
        let table = read_txn.open_table(table_kind.def()).ok()?;
        let val = table.get(key).ok()??;
        let entry: CachedEntry<T> = serde_json::from_slice(val.value()).ok()?;
        if now_unix_secs().saturating_sub(entry.cached_at) > ttl.as_secs() {
            return None;
        }
        Some(entry.value)
    }

    pub(super) fn set_cached_json<T: Serialize + ?Sized>(
        &self,
        table_kind: CacheTable,
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
            let mut table = write_txn.open_table(table_kind.def())?;
            table.insert(key, bytes.as_slice())?;
        }
        write_txn.commit()?;
        Ok(())
    }

    pub(super) fn remove_cached_entry(&self, table_kind: CacheTable, key: &str) -> Result<()> {
        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(table_kind.def())?;
            let _ = table.remove(key)?;
        }
        write_txn.commit()?;
        Ok(())
    }

    pub(super) fn get_cached_summary(
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

    pub(super) fn clear_cache_tables(&self) -> Result<()> {
        let write_txn = self.db.begin_write()?;
        delete_cache_tables(&write_txn);
        init_schema_in_txn(&write_txn)?;
        write_txn.commit()?;
        Ok(())
    }

    pub(super) fn prune_cache_by_age(&self, max_age: Duration) -> Result<(usize, u64)> {
        self.prune_cache_with_ttls(max_age, max_age, max_age, Some(max_age))
    }

    pub(super) fn prune_cache_with_ttls(
        &self,
        pages_ttl: Duration,
        search_ttl: Duration,
        caps_ttl: Duration,
        summaries_ttl: Option<Duration>,
    ) -> Result<(usize, u64)> {
        let now = now_unix_secs();
        let mut pruned = 0;
        let mut reclaimed = 0u64;

        let write_txn = self.db.begin_write()?;
        for (def, ttl) in [
            (TABLE_CACHE_PAGES, pages_ttl),
            (TABLE_CACHE_SEARCH, search_ttl),
            (TABLE_CACHE_CAPS, caps_ttl),
        ] {
            let (p, r) = prune_table_older_than(&write_txn, def, ttl.as_secs(), now)?;
            pruned += p;
            reclaimed = reclaimed.saturating_add(r);
        }

        if let Some(sum_ttl) = summaries_ttl {
            let max_sum_secs = sum_ttl.as_secs();
            let mut ck_table = write_txn.open_table(TABLE_CACHE_KEY_TO_ID)?;
            let hist_table = write_txn.open_table(TABLE_HISTORY)?;
            let mut stale_keys = Vec::new();
            for item_res in ck_table.iter()? {
                let (k, id_val) = item_res?;
                let expired = match hist_table.get(id_val.value())? {
                    Some(v) => serde_json::from_slice::<LibraryItem>(v.value())
                        .ok()
                        .and_then(|it| parse_iso_to_unix_secs(&it.created_at))
                        .is_none_or(|ts| now.saturating_sub(ts) > max_sum_secs),
                    None => true,
                };
                if expired {
                    stale_keys.push((k.value().to_string(), (k.value().len() + 8) as u64));
                }
            }
            for (k, bytes) in stale_keys {
                let _ = ck_table.remove(k.as_str())?;
                pruned += 1;
                reclaimed = reclaimed.saturating_add(bytes);
            }
        }
        write_txn.commit()?;
        Ok((pruned, reclaimed))
    }

    pub(super) fn prune_cache_lru(&self, max_bytes: u64) -> Result<(usize, u64)> {
        let write_txn = self.db.begin_write()?;
        let mut entries: Vec<(bool, String, u64, u64)> = Vec::new();
        let mut total_bytes = 0u64;
        {
            let mut pages = write_txn.open_table(TABLE_CACHE_PAGES)?;
            let mut search = write_txn.open_table(TABLE_CACHE_SEARCH)?;
            for (is_pages, table) in [(true, &pages), (false, &search)] {
                for item_res in table.iter()? {
                    let (k, v) = item_res?;
                    let size = (k.value().len() + v.value().len()) as u64;
                    total_bytes = total_bytes.saturating_add(size);
                    let cached_at = serde_json::from_slice::<CachedEntry<IgnoredAny>>(v.value())
                        .map(|r| r.cached_at)
                        .unwrap_or(0);
                    entries.push((is_pages, k.value().to_string(), cached_at, size));
                }
            }

            if total_bytes <= max_bytes || entries.is_empty() {
                return Ok((0, 0));
            }

            entries.sort_by_key(|e| e.2);
            let mut pruned = 0;
            let mut reclaimed = 0u64;
            let mut cur_bytes = total_bytes;
            for (is_pages, key, _, size) in entries {
                if cur_bytes <= max_bytes {
                    break;
                }
                let table = if is_pages { &mut pages } else { &mut search };
                let _ = table.remove(key.as_str())?;
                cur_bytes = cur_bytes.saturating_sub(size);
                reclaimed = reclaimed.saturating_add(size);
                pruned += 1;
            }
            drop((pages, search));
            write_txn.commit()?;
            Ok((pruned, reclaimed))
        }
    }

    pub(super) fn cache_stats_breakdown(&self) -> (usize, u64, usize, u64, usize, u64) {
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
                    bytes = bytes.saturating_add((r.0.value().len() + r.1.value().len()) as u64);
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
                    sum_bytes = sum_bytes.saturating_add(item.summary.len() as u64);
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
    pub(super) fn backdate_for_test(&self, target: &str, delta_secs: u64) {
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
                    if rec.value.url != target && k.value() != target {
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
        if let Ok(mut table) = write_txn.open_table(TABLE_HISTORY) {
            let updates: Vec<_> = table
                .iter()
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|(k, v)| {
                    let mut item = serde_json::from_slice::<LibraryItem>(v.value()).ok()?;
                    if item.title != target && item.url.as_deref() != Some(target) {
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
    if days < UNIX_EPOCH_DAYS {
        return None;
    }
    let unix_days = (days - UNIX_EPOCH_DAYS) as u64;
    let mut secs = unix_days * 86400;
    if s.len() >= 16 && (s.as_bytes()[10] == b'T' || s.as_bytes()[10] == b' ') {
        let h: u64 = s[11..13].parse().ok()?;
        let m: u64 = s[14..16].parse().ok()?;
        let s_part: u64 = s.get(17..19).and_then(|v| v.parse().ok()).unwrap_or(0);
        if h >= 24 || m >= 60 || s_part >= 60 {
            return None;
        }
        secs += h * 3600 + m * 60 + s_part;
    }
    Some(secs)
}

fn find_case_insensitive_offset(text: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    if needle.is_ascii() {
        return crate::system::text_util::find_ascii_case_insensitive(text, needle);
    }
    let lower_needle = needle.to_lowercase();
    for (idx, _) in text.char_indices() {
        if text[idx..]
            .chars()
            .flat_map(char::to_lowercase)
            .zip(lower_needle.chars())
            .filter(|(a, b)| a == b)
            .count()
            == lower_needle.chars().count()
        {
            return Some(idx);
        }
    }
    None
}

fn extract_snippet(text: &str, query: &str, terms: &[String]) -> String {
    let match_pos = find_case_insensitive_offset(text, query).or_else(|| {
        terms
            .iter()
            .find_map(|t| find_case_insensitive_offset(text, t))
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
    let z = (secs / 86400) as i64 + UNIX_EPOCH_DAYS;
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

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 => {
            if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) {
                29
            } else {
                28
            }
        }
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

fn ymd_to_day_number(s: &str) -> Option<i64> {
    let mut parts = s.split('-');
    let y: i64 = parts.next()?.parse().ok()?;
    let m: i64 = parts.next()?.parse().ok()?;
    let d: i64 = parts.next()?.parse().ok()?;
    if parts.next().is_some()
        || !(1970..=9999).contains(&y)
        || !(1..=12).contains(&m)
        || !(1..=days_in_month(y, m)).contains(&d)
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
    fn test_calendar_date_conversion_and_validation() {
        for ymd in [
            "1970-01-01",
            "2000-02-29",
            "2024-02-29",
            "2026-03-01",
            "2100-02-28",
        ] {
            let days = ymd_to_day_number(ymd).expect("valid ymd");
            let unix_secs = ((days - UNIX_EPOCH_DAYS) as u64) * 86400;
            assert_eq!(format_date_from_timestamp(unix_secs), ymd);
        }
        for bad in ["2026-02-29", "2026-04-31", "99999-01-01"] {
            assert!(ymd_to_day_number(bad).is_none(), "{bad}");
        }
        assert!(parse_iso_to_unix_secs("2026-01-01T25:00:00Z").is_none());
        assert!(parse_iso_to_unix_secs("2026年02月28").is_none());
    }

    #[test]
    fn test_format_csv_cell_blocks_formula_bypasses() {
        for dangerous in [
            "\t=cmd|'/C calc'!A0",
            "\r=1+1",
            "\t123",
            "  =SUM(A1:A2)",
            "+inf",
            "-NaN",
            "--42",
        ] {
            let formatted = format_csv_cell(dangerous);
            let unquoted = formatted.strip_prefix('"').unwrap_or(&formatted);
            assert!(unquoted.starts_with('\''), "payload: {dangerous:?}");
        }
        assert_eq!(format_csv_cell("-42.5"), "-42.5");
        assert_eq!(format_csv_cell("hello, world"), "\"hello, world\"");
    }

    #[test]
    fn test_summary_cache_key_disambiguates_styles_and_models() {
        let db = Database::open_in_memory().unwrap();
        let url = "https://example.com/article";
        for (style, text) in [
            ("concise", "Concise summary"),
            ("detailed", "Detailed summary"),
        ] {
            db.record_summary(
                &NewSummaryRecord {
                    style,
                    model: "m1",
                    ..make_summary("web", "Title", None, Some(url), text)
                },
                Some(url),
            )
            .unwrap();
        }
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
    fn test_database_crud_stats_and_reset() {
        let db = Database::open_in_memory().unwrap();
        db.record_summary(
            &NewSummaryRecord {
                style: "detailed",
                word_count: 10000,
                prompt_tokens: Some(5000),
                completion_tokens: Some(1500),
                duration_ms: 3000,
                tokens_per_second: Some(50.0),
                reasoning: Some("Ecological themes."),
                ..make_summary(
                    "book",
                    "Dune",
                    Some("Frank Herbert"),
                    Some("https://en.wikipedia.org/wiki/Dune"),
                    "Dune is a sci-fi novel.",
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
                    "Rust Lang",
                    None,
                    Some("https://example.com/rust"),
                    "Systems programming.",
                )
            },
            None,
        )
        .unwrap();

        let history = db.get_history(10).unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].title, "Rust Lang");
        assert_eq!(db.get_latest_summary().unwrap().unwrap().title, "Rust Lang");

        let stats = db.get_stats().unwrap();
        assert_eq!(stats.total_summaries, 2);
        assert_eq!((stats.book_summaries, stats.web_summaries), (1, 1));
        assert_eq!(stats.total_source_words, 12000);
        assert_eq!(stats.reads_today, 2);
        assert_eq!(stats.current_streak_days, 1);
        assert_eq!(stats.models[0].runs, 2);

        let items = db.get_all_library_items().unwrap();
        assert_eq!(items[1].reasoning.as_deref(), Some("Ecological themes."));
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
                Some("https://example.com/page?a=1\nmalicious_key: true"),
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
        assert!(
            std::fs::read_to_string(&csv_path)
                .unwrap()
                .contains("Summary content 2")
        );

        let vault_dir = tmp.path().join("vault");
        assert_eq!(db.export_markdown_vault(&vault_dir).unwrap(), 2);
        for entry in std::fs::read_dir(&vault_dir).unwrap() {
            let content = std::fs::read_to_string(entry.unwrap().path()).unwrap();
            let frontmatter = content.split("---").nth(1).expect("frontmatter");
            assert!(!frontmatter.contains("\nInjectedField:"));
            assert!(!frontmatter.contains("\nmalicious_key:"));
            assert!(frontmatter.contains("\\\"Quotes\\\""));
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
    fn test_database_search_and_library_item_matching() {
        let db = Database::open_in_memory().unwrap();
        db.record_summary(
            &make_summary(
                "web",
                "Rust Compiler Deep Dive",
                Some("Compiler Team"),
                Some("https://example.com/rust-compiler"),
                "The rust compiler achieves zero-cost abstractions.",
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
                "Story of Jean Valjean, rust compiler, and 狐狸 🌟.",
            ),
            None,
        )
        .unwrap();

        let hits = db.search("Rust Compiler").unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].title, "Rust Compiler Deep Dive");
        assert!(hits[0].score.unwrap() > hits[1].score.unwrap());
        assert_eq!(db.search("Victor Hugo").unwrap().len(), 1);
        assert!(db.search("狐狸").unwrap()[0].snippet.contains("狐狸"));
        assert!(!db.search("MISÉRABLES").unwrap()[0].snippet.is_empty());
        assert!(db.search("").unwrap().is_empty());

        let item = db.get_latest_summary().unwrap().unwrap();
        assert!(item.matches_query("misérables"));
        assert!(item.matches_query("hugo"));
        assert!(!item.matches_query("sandworms"));
    }

    #[test]
    fn test_database_chat_sessions_lifecycle() {
        let db = Database::open_in_memory().unwrap();
        let session_id = db
            .create_chat_session("Rust Guide", "web", Some("https://example.com"), "gemma-4")
            .unwrap();
        assert!(session_id > 0);
        assert!(db.add_chat_turn(-1, "q", "a").is_err());

        db.add_chat_turn(session_id, "Arc vs Rc?", "Arc is atomic.")
            .unwrap();
        let sessions = db.get_chat_sessions(10).unwrap();
        assert_eq!((sessions.len(), sessions[0].message_count), (1, 2));
        assert!(sessions[0].messages.is_empty());

        let (_, messages) = db.get_chat_session(session_id).unwrap().unwrap();
        assert_eq!(messages.len(), 2);
        assert!(db.delete_chat_session(session_id).unwrap());
        assert!(db.get_chat_session(session_id).unwrap().is_none());
    }

    #[test]
    fn test_database_file_open_reopen_and_symlink_rejection() {
        let tmp = crate::fs_util::tempdir().unwrap();
        let db_path = tmp.path().join("reopen.db");
        {
            let db = Database::open_at(&db_path).unwrap();
            db.record_summary(&make_summary("book", "Reopen", None, None, "Body"), None)
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
