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
    };

    db.record_summary(&record).unwrap();

    let history = db.get_history(10).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].title, "Dune");
    assert_eq!(history[0].author.as_deref(), Some("Frank Herbert"));
    assert_eq!(history[0].word_count, 11106);
    assert_eq!(history[0].prompt_tokens, Some(5200));
    assert_eq!(history[0].completion_tokens, Some(1840));
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
    let db_path = std::env::temp_dir().join(format!(
        "srz_test_db_reopen_{}.duckdb",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _cleanup = CleanFile(&db_path);

    struct CleanFile<'a>(&'a std::path::Path);
    impl<'a> Drop for CleanFile<'a> {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(self.0);
        }
    }

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
}
