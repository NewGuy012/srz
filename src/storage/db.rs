use crate::cache::Cache;
use crate::fs_util;
use anyhow::{Context, Result};
use duckdb::{Connection, params};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const DUCKDB_SECURITY_LOCKDOWN: &str =
    "SET autoinstall_known_extensions = false; SET autoload_known_extensions = false;";

pub struct NewSummaryRecord<'a> {
    pub target_type: &'a str, // 'book' or 'web'
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
pub struct ModelStats {
    pub model: String,
    pub runs: i64,
    pub total_completion_tokens: i64,
    pub avg_tokens_per_second: f64,
    pub p50_tokens_per_second: f64,
    pub p95_tokens_per_second: f64,
    pub avg_duration_ms: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainStats {
    pub domain: String,
    pub count: i64,
    pub total_words: i64,
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
pub struct LibraryItem {
    pub id: i64,
    pub target_type: String,
    pub title: String,
    pub author: Option<String>,
    pub url: Option<String>,
    pub summary: String,
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
    /// Returns true if this item's title, author, url, summary, or target type matches `query` (case-insensitively).
    /// Optimized to check short metadata fields first and avoid heap allocations.
    pub fn matches_query(&self, query: &str) -> bool {
        let q = query.trim();
        if q.is_empty() {
            return true;
        }
        if q.is_ascii() {
            crate::search::contains_ascii_case_insensitive(&self.title, q)
                || crate::search::contains_ascii_case_insensitive(&self.target_type, q)
                || self
                    .author
                    .as_deref()
                    .is_some_and(|a| crate::search::contains_ascii_case_insensitive(a, q))
                || self
                    .url
                    .as_deref()
                    .is_some_and(|u| crate::search::contains_ascii_case_insensitive(u, q))
                || crate::search::contains_ascii_case_insensitive(&self.summary, q)
        } else {
            let q_lower = q.to_lowercase();
            let matches_field = |field: &str| -> bool {
                if field.is_ascii() {
                    false
                } else {
                    field.to_lowercase().contains(&q_lower)
                }
            };

            matches_field(&self.title)
                || self.author.as_deref().is_some_and(matches_field)
                || matches_field(&self.summary)
        }
    }
}

pub struct Database {
    conn: Connection,
}

impl Database {
    /// Returns the default path for the DuckDB database (`~/.cache/srz/srz.duckdb`).
    pub fn default_path() -> PathBuf {
        Cache::default_cache_dir().join("srz.duckdb")
    }

    /// Opens the default DuckDB database located at `~/.cache/srz/srz.duckdb`.
    pub fn open_default() -> Result<Self> {
        let db_path = Self::default_path();
        if let Some(base_dir) = db_path.parent() {
            fs_util::create_private_dir_all(base_dir)
                .with_context(|| format!("Failed to create directory {:?}", base_dir))?;
        }
        Self::open_at(&db_path)
    }

    /// Opens a DuckDB database at the specified file path.
    pub fn open_at(path: &Path) -> Result<Self> {
        let exists = path.exists()
            && std::fs::metadata(path)
                .map(|m| m.len() > 0)
                .unwrap_or(false);
        let conn = Connection::open(path)
            .with_context(|| format!("Failed to open DuckDB database at {:?}", path))?;
        if path.exists() {
            fs_util::set_private_file_perms(path);
        }
        let _ = conn.execute_batch(DUCKDB_SECURITY_LOCKDOWN);
        let db = Self { conn };
        if !exists || !db.is_schema_initialized() {
            db.init_schema()?;
        }
        Ok(db)
    }

    /// Opens an in-memory DuckDB database (useful for testing).
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().context("Failed to open in-memory DuckDB")?;
        let _ = conn.execute_batch(DUCKDB_SECURITY_LOCKDOWN);
        let db = Self { conn };
        db.init_schema()?;
        Ok(db)
    }

    fn is_schema_initialized(&self) -> bool {
        self.conn
            .execute("SELECT 1 FROM history LIMIT 0", [])
            .is_ok()
    }

    fn init_schema(&self) -> Result<()> {
        self.conn
            .execute_batch(
                "CREATE SEQUENCE IF NOT EXISTS history_id_seq;
             CREATE TABLE IF NOT EXISTS history (
                 id INTEGER DEFAULT nextval('history_id_seq') PRIMARY KEY,
                 target_type VARCHAR NOT NULL,
                 title VARCHAR NOT NULL,
                 author VARCHAR,
                 url VARCHAR,
                 style VARCHAR NOT NULL,
                 model VARCHAR NOT NULL,
                 word_count UINTEGER NOT NULL,
                 prompt_tokens UINTEGER,
                 completion_tokens UINTEGER,
                 duration_ms UINTEGER NOT NULL,
                 tokens_per_second FLOAT,
                 created_at TIMESTAMP NOT NULL DEFAULT current_timestamp
             );

             CREATE SEQUENCE IF NOT EXISTS library_id_seq;
             CREATE TABLE IF NOT EXISTS library (
                 id INTEGER DEFAULT nextval('library_id_seq') PRIMARY KEY,
                 target_type VARCHAR NOT NULL,
                 title VARCHAR NOT NULL,
                 author VARCHAR,
                 url VARCHAR,
                 summary TEXT NOT NULL,
                 created_at TIMESTAMP NOT NULL DEFAULT current_timestamp
             );

             CREATE TABLE IF NOT EXISTS library_terms (
                 doc_id INTEGER NOT NULL,
                 term VARCHAR NOT NULL,
                 term_freq INTEGER NOT NULL,
                 in_title BOOLEAN NOT NULL DEFAULT FALSE,
                 PRIMARY KEY (doc_id, term)
             );

             CREATE SEQUENCE IF NOT EXISTS chat_sessions_id_seq;
             CREATE TABLE IF NOT EXISTS chat_sessions (
                 id INTEGER DEFAULT nextval('chat_sessions_id_seq') PRIMARY KEY,
                 title VARCHAR NOT NULL,
                 target_type VARCHAR NOT NULL,
                 target_url VARCHAR,
                 model VARCHAR NOT NULL,
                 created_at TIMESTAMP NOT NULL DEFAULT current_timestamp,
                 updated_at TIMESTAMP NOT NULL DEFAULT current_timestamp
             );

             CREATE SEQUENCE IF NOT EXISTS chat_messages_id_seq;
             CREATE TABLE IF NOT EXISTS chat_messages (
                 id INTEGER DEFAULT nextval('chat_messages_id_seq') PRIMARY KEY,
                 session_id INTEGER NOT NULL,
                 role VARCHAR NOT NULL,
                 content TEXT NOT NULL,
                 created_at TIMESTAMP NOT NULL DEFAULT current_timestamp
             );",
            )
            .context("Failed to initialize DuckDB schema")?;

        self.backfill_terms_if_needed()?;
        Ok(())
    }

    fn backfill_terms_if_needed(&self) -> Result<()> {
        let unindexed_count: i64 = self.conn.query_row(
            "SELECT count(*) FROM library l WHERE NOT EXISTS (SELECT 1 FROM library_terms lt WHERE lt.doc_id = l.id)",
            [],
            |row| row.get(0),
        ).unwrap_or(0);

        if unindexed_count > 0 {
            let mut stmt = self.conn.prepare(
                "SELECT l.id, l.title, l.summary FROM library l WHERE NOT EXISTS (SELECT 1 FROM library_terms lt WHERE lt.doc_id = l.id)"
            )?;
            let items: Vec<(i64, String, String)> = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect::<duckdb::Result<Vec<_>>>()?;

            self.with_transaction(|conn| {
                for (id, title, summary) in items {
                    insert_doc_terms(conn, id, &title, &summary)?;
                }
                Ok(())
            })?;
        }
        Ok(())
    }

    /// Resets all DuckDB tables and sequences, clearing reading history and the search library.
    pub fn reset(&self) -> Result<()> {
        self.conn
            .execute_batch(
                "DROP TABLE IF EXISTS chat_messages;
             DROP TABLE IF EXISTS chat_sessions;
             DROP TABLE IF EXISTS library_terms;
             DROP TABLE IF EXISTS library;
             DROP TABLE IF EXISTS history;
             DROP SEQUENCE IF EXISTS chat_messages_id_seq;
             DROP SEQUENCE IF EXISTS chat_sessions_id_seq;
             DROP SEQUENCE IF EXISTS history_id_seq;
             DROP SEQUENCE IF EXISTS library_id_seq;",
            )
            .context("Failed to drop DuckDB tables")?;

        self.init_schema()?;
        Ok(())
    }

    /// Executes a closure within a single DuckDB transaction, automatically rolling back on error.
    pub fn with_transaction<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        self.conn
            .execute_batch("BEGIN TRANSACTION;")
            .context("Failed to start DuckDB transaction")?;

        match f(&self.conn) {
            Ok(res) => {
                self.conn
                    .execute_batch("COMMIT;")
                    .context("Failed to commit DuckDB transaction")?;
                Ok(res)
            }
            Err(err) => {
                let _ = self.conn.execute_batch("ROLLBACK;");
                Err(err)
            }
        }
    }

    /// Records a newly generated summary into both the `history` and `library` tables within a single transaction.
    pub fn record_summary(&self, record: &NewSummaryRecord<'_>) -> Result<()> {
        self.with_transaction(|conn| {
            conn.execute(
                "INSERT INTO history (
                    target_type, title, author, url, style, model,
                    word_count, prompt_tokens, completion_tokens,
                    duration_ms, tokens_per_second
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    record.target_type,
                    record.title,
                    record.author,
                    record.url,
                    record.style,
                    record.model,
                    record.word_count as u32,
                    record.prompt_tokens,
                    record.completion_tokens,
                    record.duration_ms as u32,
                    record.tokens_per_second,
                ],
            )
            .context("Failed to record entry in DuckDB history table")?;

            conn.execute(
                "INSERT INTO library (id, target_type, title, author, url, summary)
                 VALUES (currval('history_id_seq'), ?, ?, ?, ?, ?)",
                params![
                    record.target_type,
                    record.title,
                    record.author,
                    record.url,
                    record.summary,
                ],
            )
            .context("Failed to record entry in DuckDB library table")?;

            let doc_id: i64 =
                conn.query_row("SELECT currval('history_id_seq')", [], |r| r.get(0))?;
            insert_doc_terms(conn, doc_id, record.title, record.summary)?;

            Ok(())
        })
    }

    /// Fetches the most recent `limit` entries from the summarization history.
    pub fn get_history(&self, limit: usize) -> Result<Vec<HistoryEntry>> {
        let sql = format!("{SELECT_HISTORY_SQL} LIMIT ?");
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![limit as i64], map_history_row)?;
        rows.collect::<duckdb::Result<Vec<_>>>().map_err(Into::into)
    }

    /// Fetches all records from history ordered descending by ID for the library index view.
    pub fn get_all_for_index(&self) -> Result<Vec<HistoryEntry>> {
        let mut stmt = self.conn.prepare(SELECT_HISTORY_SQL)?;
        let rows = stmt.query_map([], map_history_row)?;
        rows.collect::<duckdb::Result<Vec<_>>>().map_err(Into::into)
    }

    /// Fetches all library items joined with history metadata for the interactive TUI browser.
    pub fn get_all_library_items(&self) -> Result<Vec<LibraryItem>> {
        let mut stmt = self.conn.prepare(SELECT_LIBRARY_ITEMS_SQL)?;
        let rows = stmt.query_map([], map_library_item_row)?;
        rows.collect::<duckdb::Result<Vec<_>>>().map_err(Into::into)
    }

    /// Fetches the single most recently generated summary item from the library.
    pub fn get_latest_summary(&self) -> Result<Option<LibraryItem>> {
        let sql = format!("{SELECT_LIBRARY_ITEMS_SQL} LIMIT 1");
        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query_map([], map_library_item_row)?;
        if let Some(row) = rows.next() {
            Ok(Some(row?))
        } else {
            Ok(None)
        }
    }

    /// Deletes a summary from both library and history tables by its ID within a single transaction.
    pub fn delete_library_item(&self, id: i64) -> Result<()> {
        self.with_transaction(|conn| {
            conn.execute("DELETE FROM library_terms WHERE doc_id = ?", params![id])?;
            conn.execute("DELETE FROM library WHERE id = ?", params![id])?;
            conn.execute("DELETE FROM history WHERE id = ?", params![id])?;
            Ok(())
        })
    }

    /// Creates a new persistent multi-turn chat session in DuckDB and returns its generated ID.
    pub fn create_chat_session(
        &self,
        title: &str,
        target_type: &str,
        target_url: Option<&str>,
        model: &str,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO chat_sessions (title, target_type, target_url, model)
             VALUES (?, ?, ?, ?)",
            params![title, target_type, target_url, model],
        )?;
        let id: i64 = self
            .conn
            .query_row("SELECT currval('chat_sessions_id_seq')", [], |r| r.get(0))?;
        Ok(id)
    }

    /// Appends a question and assistant answer turn to an existing chat session.
    pub fn add_chat_turn(&self, session_id: i64, question: &str, answer: &str) -> Result<()> {
        self.with_transaction(|conn| {
            conn.execute(
                "INSERT INTO chat_messages (session_id, role, content)
                 VALUES (?, 'user', ?)",
                params![session_id, question],
            )?;
            conn.execute(
                "INSERT INTO chat_messages (session_id, role, content)
                 VALUES (?, 'assistant', ?)",
                params![session_id, answer],
            )?;
            conn.execute(
                "UPDATE chat_sessions
                 SET updated_at = current_timestamp
                 WHERE id = ?",
                params![session_id],
            )?;
            Ok(())
        })
    }

    /// Retrieves all saved chat sessions ordered by most recently active.
    pub fn get_chat_sessions(&self, limit: usize) -> Result<Vec<ChatSession>> {
        let sql = format!(
            "{}
             GROUP BY s.id, s.title, s.target_type, s.target_url, s.model, s.created_at, s.updated_at
             ORDER BY s.updated_at DESC
             LIMIT ?",
            SELECT_CHAT_SESSIONS_BASE_SQL
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![limit as i64], map_chat_session_row)?;
        rows.collect::<duckdb::Result<Vec<_>>>().map_err(Into::into)
    }

    /// Retrieves a chat session and all its associated messages.
    pub fn get_chat_session(
        &self,
        session_id: i64,
    ) -> Result<Option<(ChatSession, Vec<ChatMessage>)>> {
        let sql = format!(
            "{}
             WHERE s.id = ?
             GROUP BY s.id, s.title, s.target_type, s.target_url, s.model, s.created_at, s.updated_at",
            SELECT_CHAT_SESSIONS_BASE_SQL
        );
        let mut session_stmt = self.conn.prepare(&sql)?;
        let mut session_rows = session_stmt.query_map(params![session_id], map_chat_session_row)?;
        let session = match session_rows.next() {
            Some(res) => res?,
            None => return Ok(None),
        };

        let mut msg_stmt = self.conn.prepare(
            "SELECT id, session_id, role, content, strftime(created_at, '%Y-%m-%d %H:%M')
             FROM chat_messages
             WHERE session_id = ?
             ORDER BY id ASC",
        )?;

        let msgs = msg_stmt
            .query_map(params![session_id], |row| {
                Ok(ChatMessage {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    role: row.get(2)?,
                    content: row.get(3)?,
                    created_at: row.get(4)?,
                })
            })?
            .collect::<duckdb::Result<Vec<_>>>()?;

        Ok(Some((session, msgs)))
    }

    /// Deletes a chat session and all its messages.
    pub fn delete_chat_session(&self, session_id: i64) -> Result<bool> {
        self.with_transaction(|conn| {
            conn.execute(
                "DELETE FROM chat_messages WHERE session_id = ?",
                params![session_id],
            )?;
            let affected = conn.execute(
                "DELETE FROM chat_sessions WHERE id = ?",
                params![session_id],
            )?;
            Ok(affected > 0)
        })
    }

    /// Computes aggregated analytics across all past summarization runs.
    pub fn get_stats(&self) -> Result<LibraryStats> {
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let today_str = format_date_from_timestamp(now_secs);
        let week_str = format_date_from_timestamp(now_secs.saturating_sub(7 * 86400));
        let month_str = format_date_from_timestamp(now_secs.saturating_sub(30 * 86400));

        let mut total_stmt = self.conn.prepare(
            "SELECT count(*),
                    coalesce(sum(case when target_type = 'book' then 1 else 0 end), 0),
                    coalesce(sum(case when target_type = 'web' then 1 else 0 end), 0),
                    coalesce(sum(case when target_type = 'file' then 1 else 0 end), 0),
                    coalesce(sum(case when target_type = 'stdin' then 1 else 0 end), 0),
                    coalesce(sum(case when target_type = 'news' then 1 else 0 end), 0),
                    coalesce(sum(case when target_type = 'youtube' then 1 else 0 end), 0),
                    coalesce(sum(word_count), 0),
                    coalesce(sum(prompt_tokens), 0),
                    coalesce(sum(completion_tokens), 0),
                    coalesce(avg(tokens_per_second), 0.0),
                    coalesce(max(tokens_per_second), 0.0),
                    coalesce(sum(duration_ms), 0),
                    coalesce(sum(case when strftime(created_at, '%Y-%m-%d') = ? then 1 else 0 end), 0),
                    coalesce(sum(case when strftime(created_at, '%Y-%m-%d') >= ? then 1 else 0 end), 0),
                    coalesce(sum(case when strftime(created_at, '%Y-%m-%d') >= ? then 1 else 0 end), 0)
             FROM history"
        )?;

        let mut stats =
            total_stmt.query_row(params![&today_str, &week_str, &month_str], |row| {
                Ok(LibraryStats {
                    total_summaries: row.get(0)?,
                    book_summaries: row.get(1)?,
                    web_summaries: row.get(2)?,
                    file_summaries: row.get(3)?,
                    stdin_summaries: row.get(4)?,
                    news_summaries: row.get(5)?,
                    video_summaries: row.get(6)?,
                    total_source_words: row.get(7)?,
                    total_summary_words: 0,
                    total_prompt_tokens: row.get(8)?,
                    total_completion_tokens: row.get(9)?,
                    avg_tokens_per_second: row.get(10)?,
                    max_tokens_per_second: row.get(11)?,
                    total_duration_ms: row.get(12)?,
                    estimated_minutes_saved: 0.0,
                    reads_today: row.get(13)?,
                    reads_this_week: row.get(14)?,
                    reads_this_month: row.get(15)?,
                    active_reading_days: 0,
                    current_streak_days: 0,
                    models: Vec::new(),
                    top_domains: Vec::new(),
                })
            })?;

        // Summary words and time saved
        let total_summary_words: i64 = self
            .conn
            .query_row(
                "SELECT coalesce(sum(length(summary) - length(replace(summary, ' ', '')) + 1), 0) FROM library",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        stats.total_summary_words = total_summary_words;
        if stats.total_source_words > total_summary_words {
            stats.estimated_minutes_saved =
                (stats.total_source_words - total_summary_words) as f64 / 225.0;
        }

        // Active days and current streak
        let mut days_stmt = self.conn.prepare(
            "SELECT DISTINCT strftime(created_at, '%Y-%m-%d') AS day_str
             FROM history
             ORDER BY day_str DESC",
        )?;
        let days: Vec<String> = days_stmt
            .query_map([], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect();

        stats.active_reading_days = days.len() as i64;
        let day_nums: Vec<i64> = days.iter().filter_map(|d| ymd_to_day_number(d)).collect();
        let today_num = ymd_to_day_number(&today_str).unwrap_or(0);

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

        // Model Performance Breakdown
        let mut model_stmt = self.conn.prepare(
            "SELECT
                 model,
                 count(*)::BIGINT AS runs,
                 coalesce(sum(completion_tokens), 0)::BIGINT AS total_completion_tokens,
                 coalesce(avg(tokens_per_second), 0.0)::DOUBLE AS avg_tps,
                 coalesce(quantile_cont(tokens_per_second, 0.50), 0.0)::DOUBLE AS p50_tps,
                 coalesce(quantile_cont(tokens_per_second, 0.95), 0.0)::DOUBLE AS p95_tps,
                 coalesce(avg(duration_ms), 0.0)::DOUBLE AS avg_duration_ms
             FROM history
             GROUP BY model
             ORDER BY runs DESC",
        )?;
        let models: Vec<ModelStats> = model_stmt
            .query_map([], |row| {
                Ok(ModelStats {
                    model: row.get(0)?,
                    runs: row.get(1)?,
                    total_completion_tokens: row.get(2)?,
                    avg_tokens_per_second: row.get(3)?,
                    p50_tokens_per_second: row.get(4)?,
                    p95_tokens_per_second: row.get(5)?,
                    avg_duration_ms: row.get(6)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        stats.models = models;

        // Top Domains
        let mut domain_stmt = self.conn.prepare(
            "SELECT
                 regexp_extract(url, '^https?://([^/:]+)', 1) AS domain,
                 count(*)::BIGINT AS count,
                 coalesce(sum(word_count), 0)::BIGINT AS total_words
             FROM history
             WHERE url IS NOT NULL AND length(regexp_extract(url, '^https?://([^/:]+)', 1)) > 0
             GROUP BY domain
             ORDER BY count DESC
             LIMIT 5",
        )?;
        let top_domains: Vec<DomainStats> = domain_stmt
            .query_map([], |row| {
                Ok(DomainStats {
                    domain: row.get(0)?,
                    count: row.get(1)?,
                    total_words: row.get(2)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        stats.top_domains = top_domains;

        Ok(stats)
    }

    /// Performs a full-text search across all summarized books and webpages in the library
    /// using BM25 relevance ranking with fallback to substring matching.
    pub fn search(&self, query: &str) -> Result<Vec<LibrarySearchResult>> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return Ok(Vec::new());
        }

        let query_terms = tokenize_terms(trimmed);
        let mut results: Vec<LibrarySearchResult> = Vec::new();
        let mut seen_ids = std::collections::HashSet::new();

        if !query_terms.is_empty() {
            let placeholders = query_terms
                .iter()
                .map(|_| "?")
                .collect::<Vec<_>>()
                .join(", ");
            let bm25_sql = format!(
                "WITH doc_stats AS (
                     SELECT COUNT(*)::DOUBLE AS total_docs,
                            COALESCE(AVG(word_count), 1.0)::DOUBLE AS avg_doc_len
                     FROM history
                 ),
                 matched_terms AS (
                     SELECT
                         lt.doc_id,
                         lt.term,
                         lt.term_freq,
                         lt.in_title,
                         h.word_count::DOUBLE AS doc_len
                     FROM library_terms lt
                     JOIN history h ON lt.doc_id = h.id
                     WHERE lt.term IN ({placeholders})
                 ),
                 term_df AS (
                     SELECT term, COUNT(DISTINCT doc_id)::DOUBLE AS df
                     FROM matched_terms
                     GROUP BY term
                 ),
                 scored_docs AS (
                     SELECT
                         mt.doc_id,
                         SUM(
                             ln(((ds.total_docs - td.df + 0.5) / (td.df + 0.5)) + 1.0) *
                             (CASE WHEN mt.in_title THEN 2.5 ELSE 1.0 END) *
                             ((mt.term_freq * 2.2) / (mt.term_freq + 1.2 * (0.25 + 0.75 * (mt.doc_len / ds.avg_doc_len))))
                         ) AS bm25_score
                     FROM matched_terms mt
                     JOIN term_df td ON mt.term = td.term
                     CROSS JOIN doc_stats ds
                     GROUP BY mt.doc_id
                 )
                 SELECT
                     l.id, l.target_type, l.title, l.author, l.url, l.summary,
                     strftime(l.created_at, '%Y-%m-%d %H:%M'),
                     sd.bm25_score
                 FROM scored_docs sd
                 JOIN library l ON sd.doc_id = l.id
                 ORDER BY sd.bm25_score DESC, l.id DESC
                 LIMIT 15"
            );

            if let Ok(mut stmt) = self.conn.prepare(&bm25_sql) {
                let params: Vec<&dyn duckdb::ToSql> = query_terms
                    .iter()
                    .map(|t| t as &dyn duckdb::ToSql)
                    .collect();
                if let Ok(rows) = stmt.query_map(params.as_slice(), |row| {
                    let score: f64 = row.get(7)?;
                    map_search_result_row(row, trimmed, &query_terms, Some(score))
                }) {
                    for r in rows.flatten() {
                        seen_ids.insert(r.id);
                        results.push(r);
                    }
                }
            }
        }

        // Fallback / complement with substring ILIKE matching if fewer than 15 results
        if results.len() < 15 {
            let mut stmt = self.conn.prepare(
                "SELECT id, target_type, title, author, url, summary,
                        strftime(created_at, '%Y-%m-%d %H:%M')
                 FROM library
                 WHERE summary ILIKE '%' || ? || '%'
                    OR title ILIKE '%' || ? || '%'
                    OR coalesce(author, '') ILIKE '%' || ? || '%'
                 ORDER BY id DESC
                 LIMIT 15",
            )?;

            let rows = stmt.query_map(params![trimmed, trimmed, trimmed], |row| {
                map_search_result_row(row, trimmed, &query_terms, None)
            })?;

            for r in rows.flatten() {
                if seen_ids.insert(r.id) {
                    results.push(r);
                    if results.len() >= 15 {
                        break;
                    }
                }
            }
        }

        Ok(results)
    }
}

/// Tokenizes text into normalized lowercased terms for BM25 indexing and querying.
pub fn tokenize_terms(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .map(|s| s.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|s| !s.is_empty())
        .map(|s| s.to_lowercase())
        .collect()
}

/// Indexes document title and summary terms into `library_terms`.
fn insert_doc_terms(conn: &Connection, doc_id: i64, title: &str, summary: &str) -> Result<()> {
    let mut term_counts: std::collections::HashMap<String, (usize, bool)> =
        std::collections::HashMap::new();

    for term in tokenize_terms(title) {
        let entry = term_counts.entry(term).or_insert((0, true));
        entry.0 += 1;
        entry.1 = true;
    }

    for term in tokenize_terms(summary) {
        let entry = term_counts.entry(term).or_insert((0, false));
        entry.0 += 1;
    }

    conn.execute(
        "DELETE FROM library_terms WHERE doc_id = ?",
        params![doc_id],
    )?;

    let mut stmt = conn.prepare(
        "INSERT INTO library_terms (doc_id, term, term_freq, in_title)
         VALUES (?, ?, ?, ?)",
    )?;

    for (term, (freq, in_title)) in term_counts {
        stmt.execute(params![doc_id, term, freq as i64, in_title])?;
    }

    Ok(())
}

const SELECT_CHAT_SESSIONS_BASE_SQL: &str =
    "SELECT s.id, s.title, s.target_type, s.target_url, s.model,
            count(m.id)::BIGINT AS msg_count,
            (SELECT m2.content FROM chat_messages m2 WHERE m2.session_id = s.id AND m2.role = 'user' ORDER BY m2.id DESC LIMIT 1) AS last_q,
            strftime(s.created_at, '%Y-%m-%d %H:%M'),
            strftime(s.updated_at, '%Y-%m-%d %H:%M')
     FROM chat_sessions s
     LEFT JOIN chat_messages m ON s.id = m.session_id";

fn map_chat_session_row(row: &duckdb::Row<'_>) -> duckdb::Result<ChatSession> {
    Ok(ChatSession {
        id: row.get(0)?,
        title: row.get(1)?,
        target_type: row.get(2)?,
        target_url: row.get(3)?,
        model: row.get(4)?,
        message_count: row.get(5)?,
        last_question: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

const SELECT_HISTORY_SQL: &str = "SELECT id, target_type, title, author, url, style, model,
            word_count, prompt_tokens, completion_tokens,
            duration_ms, tokens_per_second,
            strftime(created_at, '%Y-%m-%d %H:%M')
     FROM history
     ORDER BY id DESC";

const SELECT_LIBRARY_ITEMS_SQL: &str =
    "SELECT l.id, l.target_type, l.title, l.author, l.url, l.summary,
            coalesce(h.style, 'concise'), coalesce(h.model, 'unknown'),
            coalesce(h.word_count, 0), h.prompt_tokens, h.completion_tokens,
            coalesce(h.duration_ms, 0), h.tokens_per_second,
            strftime(l.created_at, '%Y-%m-%d %H:%M')
     FROM library l
     LEFT JOIN history h ON l.id = h.id
     ORDER BY l.id DESC";

fn map_library_item_row(row: &duckdb::Row<'_>) -> duckdb::Result<LibraryItem> {
    Ok(LibraryItem {
        id: row.get(0)?,
        target_type: row.get(1)?,
        title: row.get(2)?,
        author: row.get(3)?,
        url: row.get(4)?,
        summary: row.get(5)?,
        style: row.get(6)?,
        model: row.get(7)?,
        word_count: row.get::<_, u32>(8)?,
        prompt_tokens: row.get(9)?,
        completion_tokens: row.get(10)?,
        duration_ms: row.get::<_, u32>(11)? as u64,
        tokens_per_second: row.get(12)?,
        created_at: row.get(13)?,
    })
}

fn map_history_row(row: &duckdb::Row<'_>) -> duckdb::Result<HistoryEntry> {
    Ok(HistoryEntry {
        id: row.get(0)?,
        target_type: row.get(1)?,
        title: row.get(2)?,
        author: row.get(3)?,
        url: row.get(4)?,
        style: row.get(5)?,
        model: row.get(6)?,
        word_count: row.get(7)?,
        prompt_tokens: row.get(8)?,
        completion_tokens: row.get(9)?,
        duration_ms: row.get::<_, u32>(10)? as u64,
        tokens_per_second: row.get(11)?,
        created_at: row.get(12)?,
    })
}

fn map_search_result_row(
    row: &duckdb::Row<'_>,
    query: &str,
    terms: &[String],
    score: Option<f64>,
) -> duckdb::Result<LibrarySearchResult> {
    let id: i64 = row.get(0)?;
    let target_type: String = row.get(1)?;
    let title: String = row.get(2)?;
    let author: Option<String> = row.get(3)?;
    let url: Option<String> = row.get(4)?;
    let summary: String = row.get(5)?;
    let created_at: String = row.get(6)?;
    let snippet = extract_snippet(&summary, query, terms);

    Ok(LibrarySearchResult {
        id,
        target_type,
        title,
        author,
        url,
        snippet,
        created_at,
        score,
    })
}

/// Extracts a surrounding text excerpt (~200 characters) around the first match of `query`
/// without allocating a lowercase copy of `text`.
/// Uses UTF-8 character boundaries to prevent indexing panics on multi-byte characters.
fn extract_snippet(text: &str, query: &str, terms: &[String]) -> String {
    let match_pos = crate::search::find_ascii_case_insensitive(text, query).or_else(|| {
        for term in terms {
            if let Some(pos) = crate::search::find_ascii_case_insensitive(text, term) {
                return Some(pos);
            }
        }
        None
    });

    let (start, end) = if let Some(pos) = match_pos {
        let raw_start = pos.saturating_sub(60);
        let raw_end = (pos + query.len().min(40) + 100).min(text.len());
        (
            text.floor_char_boundary(raw_start),
            text.ceil_char_boundary(raw_end),
        )
    } else {
        let raw_end = text.len().min(160);
        (0, text.ceil_char_boundary(raw_end))
    };

    let prefix = if start > 0 { "..." } else { "" };
    let suffix = if end < text.len() { "..." } else { "" };
    let slice = text[start..end].trim();

    let mut out = String::with_capacity(prefix.len() + slice.len() + suffix.len());
    out.push_str(prefix);
    for ch in slice.chars() {
        if ch == '\n' || ch == '\r' {
            out.push(' ');
        } else {
            out.push(ch);
        }
    }
    out.push_str(suffix);
    out
}

/// Converts Unix epoch seconds to UTC date string formatted as `YYYY-MM-DD`.
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

/// Converts a `YYYY-MM-DD` date string to continuous day number for streak comparisons.
fn ymd_to_day_number(s: &str) -> Option<i64> {
    let mut parts = s.split('-');
    let y: i64 = parts.next()?.parse().ok()?;
    let m: i64 = parts.next()?.parse().ok()?;
    let d: i64 = parts.next()?.parse().ok()?;
    let m = (m + 9) % 12;
    let y = y - m / 10;
    Some(365 * y + y / 4 - y / 100 + y / 400 + (m * 306 + 5) / 10 + (d - 1))
}
