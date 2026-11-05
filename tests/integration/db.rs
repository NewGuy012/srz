use srz::db::{Database, NewSummaryRecord};

#[test]
fn test_database_record_and_history() {
    let db = Database::open_in_memory().unwrap();

    let record = NewSummaryRecord {
        target_type: "book",
        title: "Dune",
        author: Some("Frank Herbert"),
        url: Some("https://en.wikipedia.org/wiki/Dune_(novel)"),
        style: "detailed",
        model: "gemma-4",
        word_count: 11106,
        prompt_tokens: Some(5200),
        completion_tokens: Some(1840),
        duration_ms: 4200,
        tokens_per_second: Some(43.8),
        summary: "Dune is a 1965 epic sci-fi novel about Arrakis, ecology, and Melange spice.",
        reasoning: Some("Analyzed Herbert's ecological themes."),
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
fn test_database_query_raw_and_exports() {
    let db = Database::open_in_memory().unwrap();

    let record = NewSummaryRecord {
        target_type: "book",
        title: "Dune",
        author: Some("Frank Herbert"),
        url: Some("https://en.wikipedia.org/wiki/Dune_(novel)"),
        style: "detailed",
        model: "gemma-4",
        word_count: 11106,
        prompt_tokens: Some(5200),
        completion_tokens: Some(1840),
        duration_ms: 4200,
        tokens_per_second: Some(43.8),
        summary: "Dune is a 1965 epic sci-fi novel about Arrakis.",
        reasoning: None,
    };
    db.record_summary(&record).unwrap();

    // 1. query_raw
    let q_res = db
        .query_raw("SELECT target_type, count(*) as cnt FROM history GROUP BY target_type")
        .unwrap();
    assert_eq!(q_res.columns, vec!["target_type", "cnt"]);
    assert_eq!(q_res.rows.len(), 1);
    assert_eq!(q_res.rows[0][0], serde_json::json!("book"));
    assert_eq!(q_res.rows[0][1], serde_json::json!(1));

    // 2. export_json
    let tmp = tempfile::tempdir().unwrap();
    let json_path = tmp.path().join("export.json");
    db.export_json(&json_path).unwrap();
    assert!(json_path.exists());
    let json_str = std::fs::read_to_string(&json_path).unwrap();
    assert!(json_str.contains("Dune"));

    // 3. export_markdown_vault
    let vault_dir = tmp.path().join("vault");
    let count = db.export_markdown_vault(&vault_dir).unwrap();
    assert_eq!(count, 1);
    let md_file = vault_dir.join("dune.md");
    assert!(md_file.exists());
    let md_content = std::fs::read_to_string(&md_file).unwrap();
    assert!(md_content.starts_with("---\n"));
    assert!(md_content.contains("author: \"Frank Herbert\""));
    assert!(md_content.contains("# Dune"));
    assert!(md_content.contains("Arrakis"));

    // 4. export_csv
    let csv_path = tmp.path().join("export.csv");
    db.export_csv(&csv_path).unwrap();
    assert!(csv_path.exists());
    let csv_str = std::fs::read_to_string(&csv_path).unwrap();
    assert!(csv_str.contains("Dune"));
    assert!(csv_str.contains("Frank Herbert"));

    // 5. handle_query with multi-byte characters and ANSI sequences
    let long_multibyte_sql = "SELECT '✨🌟🚀 ' || repeat('狐狸日本語 café résumé — ', 5) as mb_col, '\x1b[31;1mInjected ANSI\x1b[0m' as ansi_col";
    srz::commands::handle_query(&db, long_multibyte_sql, false, false).unwrap();
    srz::commands::handle_query(&db, long_multibyte_sql, true, false).unwrap();
    srz::commands::handle_query(&db, long_multibyte_sql, false, true).unwrap();
}

#[test]
fn test_database_stats() {
    let db = Database::open_in_memory().unwrap();

    let r1 = NewSummaryRecord {
        target_type: "book",
        title: "Dune",
        author: Some("Frank Herbert"),
        url: None,
        style: "detailed",
        model: "gemma-4",
        word_count: 10000,
        prompt_tokens: Some(5000),
        completion_tokens: Some(1500),
        duration_ms: 3000,
        tokens_per_second: Some(50.0),
        summary: "Book summary 1",
        reasoning: None,
    };
    db.record_summary(&r1).unwrap();

    let r2 = NewSummaryRecord {
        target_type: "web",
        title: "Rust Article",
        author: None,
        url: Some("https://example.com/rust"),
        style: "concise",
        model: "gemma-4",
        word_count: 2000,
        prompt_tokens: Some(2500),
        completion_tokens: Some(500),
        duration_ms: 1000,
        tokens_per_second: Some(50.0),
        summary: "Web summary 2",
        reasoning: None,
    };
    db.record_summary(&r2).unwrap();

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
fn test_database_search_library() {
    let db = Database::open_in_memory().unwrap();

    let record = NewSummaryRecord {
        target_type: "book",
        title: "Dune",
        author: Some("Frank Herbert"),
        url: Some("https://en.wikipedia.org/wiki/Dune_(novel)"),
        style: "detailed",
        model: "gemma-4",
        word_count: 11106,
        prompt_tokens: Some(5200),
        completion_tokens: Some(1840),
        duration_ms: 4200,
        tokens_per_second: Some(43.8),
        summary: "Set on the harsh desert planet Arrakis, the sole source of Melange spice in the known universe.",
        reasoning: None,
    };
    db.record_summary(&record).unwrap();

    let hits = db.search("Melange").unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].title, "Dune");
    assert!(hits[0].snippet.contains("Melange"));

    let no_hits = db.search("Cyberpunk").unwrap();
    assert!(no_hits.is_empty());
}

#[test]
fn test_database_search_multibyte_utf8_snippet_safety() {
    let db = Database::open_in_memory().unwrap();

    // Text with multi-byte UTF-8 characters (emojis, accents, CJK)
    let record = NewSummaryRecord {
        target_type: "book",
        title: "Le Petit Prince 👑",
        author: Some("Antoine de Saint-Exupéry"),
        url: None,
        style: "concise",
        model: "gemma-4",
        word_count: 500,
        prompt_tokens: Some(300),
        completion_tokens: Some(150),
        duration_ms: 800,
        tokens_per_second: Some(35.0),
        summary: "🌟 Voici mon secret. Il est très simple: on ne voit bien qu’avec le cœur. L’essentiel est invisible pour les yeux. 🚀🪐✨ 狐狸 and roses.",
        reasoning: None,
    };
    db.record_summary(&record).unwrap();

    let hits = db.search("secret").unwrap();
    assert_eq!(hits.len(), 1);
    assert!(hits[0].snippet.contains("secret"));

    let hits_cjk = db.search("狐狸").unwrap();
    assert_eq!(hits_cjk.len(), 1);
    assert!(hits_cjk[0].snippet.contains("狐狸"));
}

#[test]
fn test_database_search_bm25_relevance_ranking() {
    let db = Database::open_in_memory().unwrap();

    let doc_a = NewSummaryRecord {
        target_type: "web",
        title: "Rust Compiler Deep Dive",
        author: Some("Compiler Team"),
        url: Some("https://example.com/rust-compiler"),
        style: "detailed",
        model: "gemma-4",
        word_count: 3500,
        prompt_tokens: Some(2000),
        completion_tokens: Some(800),
        duration_ms: 1200,
        tokens_per_second: Some(40.0),
        summary: "The rust compiler achieves zero-cost abstractions and memory safety through borrow checker analysis. Rust compiler optimizations are extensive.",
        reasoning: None,
    };
    db.record_summary(&doc_a).unwrap();

    let doc_b = NewSummaryRecord {
        target_type: "web",
        title: "General Software Patterns",
        author: Some("John Doe"),
        url: Some("https://example.com/patterns"),
        style: "concise",
        model: "gemma-4",
        word_count: 5000,
        prompt_tokens: Some(3000),
        completion_tokens: Some(400),
        duration_ms: 800,
        tokens_per_second: Some(40.0),
        summary: "Design patterns in systems programming. Briefly mentions rust compiler in passing.",
        reasoning: None,
    };
    db.record_summary(&doc_b).unwrap();

    let hits = db.search("Rust Compiler").unwrap();
    assert_eq!(hits.len(), 2);
    // doc_a has "Rust Compiler" in title and multiple times in summary -> higher BM25 score
    assert_eq!(hits[0].title, "Rust Compiler Deep Dive");
    assert!(hits[0].score.is_some());
    assert!(hits[1].score.is_some());
    assert!(hits[0].score.unwrap() > hits[1].score.unwrap());
    assert!(hits[0].snippet.to_lowercase().contains("rust"));
}

#[test]
fn test_database_library_items_and_deletion() {
    let db = Database::open_in_memory().unwrap();

    let record1 = NewSummaryRecord {
        target_type: "book",
        title: "Dune",
        author: Some("Frank Herbert"),
        url: None,
        style: "concise",
        model: "gemma-4",
        word_count: 1000,
        prompt_tokens: Some(500),
        completion_tokens: Some(250),
        duration_ms: 1200,
        tokens_per_second: Some(45.0),
        summary: "Spice on Arrakis.",
        reasoning: None,
    };
    db.record_summary(&record1).unwrap();

    let record2 = NewSummaryRecord {
        target_type: "web",
        title: "Rust Lang",
        author: None,
        url: Some("https://rust-lang.org"),
        style: "detailed",
        model: "gemma-4",
        word_count: 800,
        prompt_tokens: Some(400),
        completion_tokens: Some(200),
        duration_ms: 1000,
        tokens_per_second: Some(48.0),
        summary: "Modern systems programming.",
        reasoning: None,
    };
    db.record_summary(&record2).unwrap();

    let items = db.get_all_library_items().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].title, "Rust Lang");
    assert_eq!(items[1].title, "Dune");

    let latest = db.get_latest_summary().unwrap();
    assert!(latest.is_some());
    assert_eq!(latest.unwrap().title, "Rust Lang");

    let id_to_delete = items[0].id;
    db.delete_library_item(id_to_delete).unwrap();

    let items_after = db.get_all_library_items().unwrap();
    assert_eq!(items_after.len(), 1);
    assert_eq!(items_after[0].title, "Dune");
}

#[test]
fn test_database_reset() {
    let db = Database::open_in_memory().unwrap();

    let record = NewSummaryRecord {
        target_type: "book",
        title: "Dune",
        author: Some("Frank Herbert"),
        url: None,
        style: "concise",
        model: "gemma-4",
        word_count: 5000,
        prompt_tokens: Some(2000),
        completion_tokens: Some(600),
        duration_ms: 1500,
        tokens_per_second: Some(40.0),
        summary: "Dune summary text",
        reasoning: None,
    };
    db.record_summary(&record).unwrap();
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
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("reopen.duckdb");

    // First open initializes schema
    {
        let db = Database::open_at(&db_path).unwrap();
        let record = NewSummaryRecord {
            target_type: "book",
            title: "Reopen Test",
            author: None,
            url: None,
            style: "concise",
            model: "gemma-4",
            word_count: 500,
            prompt_tokens: None,
            completion_tokens: None,
            duration_ms: 100,
            tokens_per_second: None,
            summary: "Reopen content",
            reasoning: None,
        };
        db.record_summary(&record).unwrap();
    }

    // Second open skips redundant init_schema and loads existing records
    {
        let db = Database::open_at(&db_path).unwrap();
        let history = db.get_history(10).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].title, "Reopen Test");
    }
}

#[test]
fn test_library_item_matches_query() {
    let db = Database::open_in_memory().unwrap();
    let record = NewSummaryRecord {
        target_type: "book",
        title: "Foundation and Empire",
        author: Some("Isaac Asimov"),
        url: Some("https://example.com/foundation"),
        style: "concise",
        model: "gemma-4",
        word_count: 500,
        prompt_tokens: None,
        completion_tokens: None,
        duration_ms: 100,
        tokens_per_second: None,
        summary: "The Mule attacks the Galactic Empire.",
        reasoning: None,
    };
    db.record_summary(&record).unwrap();
    let items = db.get_all_library_items().unwrap();
    assert_eq!(items.len(), 1);
    let item = &items[0];

    // Case-insensitive ASCII matches across various fields
    assert!(item.matches_query("foundation"));
    assert!(item.matches_query("ASIMOV"));
    assert!(item.matches_query("empire"));
    assert!(item.matches_query("mule"));
    assert!(item.matches_query("example.com"));
    assert!(item.matches_query("book"));
    assert!(item.matches_query("")); // Empty matches everything

    // Non-matches
    assert!(!item.matches_query("sandworms"));
    assert!(!item.matches_query("cyberpunk"));

    // Non-ASCII queries
    let unicode_record = NewSummaryRecord {
        target_type: "book",
        title: "Les Misérables",
        author: Some("Victor Hugo"),
        url: None,
        style: "concise",
        model: "gemma-4",
        word_count: 800,
        prompt_tokens: None,
        completion_tokens: None,
        duration_ms: 100,
        tokens_per_second: None,
        summary: "Jean Valjean and Cosette in 19th-century France.",
        reasoning: None,
    };
    db.record_summary(&unicode_record).unwrap();
    let all_items = db.get_all_library_items().unwrap();
    let unicode_item = all_items
        .iter()
        .find(|i| i.title == "Les Misérables")
        .unwrap();
    assert!(unicode_item.matches_query("Misérables"));
    assert!(unicode_item.matches_query("misérables"));
    assert!(!unicode_item.matches_query("München"));
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

    let deleted = db.delete_chat_session(session_id).unwrap();
    assert!(deleted);

    let empty = db.get_chat_session(session_id).unwrap();
    assert!(empty.is_none());

    let empty_list = db.get_chat_sessions(10).unwrap();
    assert!(empty_list.is_empty());
}

#[test]
fn test_tokenize_terms_edge_cases() {
    use srz::db::tokenize_terms;

    assert_eq!(tokenize_terms(""), Vec::<String>::new());
    assert_eq!(tokenize_terms("   \t\n  "), Vec::<String>::new());
    assert_eq!(tokenize_terms("!@#$%^&*()+=~`"), Vec::<String>::new());
    assert_eq!(
        tokenize_terms("Rust-2024_edition & AI 🤖"),
        vec!["rust-2024_edition".to_string(), "ai".to_string()]
    );
    assert_eq!(
        tokenize_terms("--flag_name--"),
        vec!["flag_name".to_string()]
    );
}

#[test]
fn test_database_search_special_characters_and_sql_injection() {
    let db = Database::open_in_memory().unwrap();

    let record = NewSummaryRecord {
        target_type: "book",
        title: "Security Handbook",
        author: Some("Alice"),
        url: None,
        style: "concise",
        model: "gemma-4",
        word_count: 500,
        prompt_tokens: None,
        completion_tokens: None,
        duration_ms: 100,
        tokens_per_second: None,
        summary: "Discussion of SQL injection vulnerabilities and escaping techniques.",
        reasoning: None,
    };
    db.record_summary(&record).unwrap();

    // SQL injection attempt string must not cause SQL syntax error
    let res = db.search("'; DROP TABLE library; --").unwrap();
    assert!(res.is_empty());

    // Table still exists and can be searched normally
    let normal_hits = db.search("vulnerabilities").unwrap();
    assert_eq!(normal_hits.len(), 1);
    assert_eq!(normal_hits[0].title, "Security Handbook");

    // Pure wildcard/punctuation queries
    assert!(db.search("% * _ ?").unwrap().is_empty());
    assert!(db.search("").unwrap().is_empty());
}

#[test]
#[cfg(unix)]
fn test_database_refuses_symlink() {
    let tmp = tempfile::tempdir().unwrap();
    let real_db = tmp.path().join("real.duckdb");
    let symlink_db = tmp.path().join("symlink.duckdb");

    // Open and initialize a real DB
    {
        let db = Database::open_at(&real_db).expect("Create real db");
        assert!(db.get_stats().is_ok());
    }

    // Create a symlink pointing to the real DB
    std::os::unix::fs::symlink(&real_db, &symlink_db).unwrap();

    // Opening the symlinked path directly must be rejected
    let res = Database::open_at(&symlink_db);
    assert!(res.is_err());
    let err = res.err().unwrap();
    assert!(err.to_string().contains("is a symlink"));
}

#[test]
fn test_database_attach() {
    let tmp = tempfile::tempdir().unwrap();
    let main_db = tmp.path().join("main.duckdb");
    let cache_db = tmp.path().join("cache.duckdb");

    let db = Database::open_at(&main_db).unwrap();
    let (cache_conn, _) = srz::storage::open_hardened_duckdb_file(&cache_db).unwrap();
    cache_conn
        .execute_batch("CREATE TABLE cache_test (id INT, val TEXT); INSERT INTO cache_test VALUES (1, 'attached');")
        .unwrap();
    drop(cache_conn);

    db.attach_database(&cache_db, "cache_schema").unwrap();
    let query_res = db
        .query_raw("SELECT val FROM cache_schema.cache_test WHERE id = 1")
        .unwrap();
    assert_eq!(query_res.rows.len(), 1);
    assert_eq!(query_res.rows[0][0], serde_json::json!("attached"));
}
