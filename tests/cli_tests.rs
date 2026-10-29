use clap::Parser;
use srz::cli::{
    CacheCommand, Cli, NewsTimeframe, ResolvedAction, SummaryStyle, clean_news_query, is_news_query,
};

#[test]
fn test_cache_subcommands() {
    let dir_cli = Cli::parse_from(["srz", "cache", "dir"]);
    assert_eq!(
        dir_cli.resolve_action().unwrap(),
        ResolvedAction::Cache(CacheCommand::Dir)
    );

    let size_cli = Cli::parse_from(["srz", "cache", "size"]);
    assert_eq!(
        size_cli.resolve_action().unwrap(),
        ResolvedAction::Cache(CacheCommand::Size)
    );

    let clean_cli = Cli::parse_from(["srz", "cache", "clean"]);
    assert_eq!(
        clean_cli.resolve_action().unwrap(),
        ResolvedAction::Cache(CacheCommand::Clean { reset_db: false })
    );

    let clean_db_cli = Cli::parse_from(["srz", "cache", "clean", "--reset-db"]);
    assert_eq!(
        clean_db_cli.resolve_action().unwrap(),
        ResolvedAction::Cache(CacheCommand::Clean { reset_db: true })
    );

    let prune_cli = Cli::parse_from([
        "srz",
        "cache",
        "prune",
        "--older-than",
        "14d",
        "--max-size",
        "500MB",
    ]);
    assert_eq!(
        prune_cli.resolve_action().unwrap(),
        ResolvedAction::Cache(CacheCommand::Prune {
            older_than: Some("14d".to_string()),
            max_size: Some("500MB".to_string()),
        })
    );
}

#[test]
fn test_history_stats_find_subcommands() {
    let hist_cli = Cli::parse_from(["srz", "history", "--limit", "20"]);
    assert_eq!(
        hist_cli.resolve_action().unwrap(),
        ResolvedAction::History { limit: 20 }
    );

    let stats_cli = Cli::parse_from(["srz", "stats"]);
    assert_eq!(stats_cli.resolve_action().unwrap(), ResolvedAction::Stats);

    let find_cli = Cli::parse_from(["srz", "find", "Melange"]);
    assert_eq!(
        find_cli.resolve_action().unwrap(),
        ResolvedAction::Find { query: "Melange" }
    );

    let models_cli = Cli::parse_from(["srz", "models"]);
    assert_eq!(models_cli.resolve_action().unwrap(), ResolvedAction::Models);

    let doctor_cli = Cli::parse_from(["srz", "doctor"]);
    assert_eq!(doctor_cli.resolve_action().unwrap(), ResolvedAction::Doctor);

    let index_cli = Cli::parse_from(["srz", "index"]);
    assert_eq!(index_cli.resolve_action().unwrap(), ResolvedAction::Index);
}

#[test]
fn test_completions_subcommand() {
    let bash_cli = Cli::parse_from(["srz", "completions", "bash"]);
    assert_eq!(
        bash_cli.resolve_action().unwrap(),
        ResolvedAction::Completions {
            shell: clap_complete::Shell::Bash
        }
    );

    let zsh_cli = Cli::parse_from(["srz", "completions", "zsh"]);
    assert_eq!(
        zsh_cli.resolve_action().unwrap(),
        ResolvedAction::Completions {
            shell: clap_complete::Shell::Zsh
        }
    );
}

#[test]
fn test_default_style_is_concise() {
    let cli = Cli::parse_from(["srz", "Dune"]);
    assert_eq!(cli.style, SummaryStyle::Concise);
}

#[test]
fn test_summary_styles() {
    let bullets_cli = Cli::parse_from(["srz", "Dune", "--style", "bullets"]);
    assert_eq!(bullets_cli.style, SummaryStyle::Bullets);
    assert_eq!(bullets_cli.style.as_str(), "bullets");

    let detailed_cli = Cli::parse_from(["srz", "Dune", "--style", "detailed"]);
    assert_eq!(detailed_cli.style, SummaryStyle::Detailed);
    assert_eq!(detailed_cli.style.as_str(), "detailed");
}

#[test]
fn test_auto_detect_url() {
    let cli = Cli::parse_from(["srz", "https://example.com/article"]);

    match cli.resolve_action().unwrap() {
        ResolvedAction::Web { url } => assert_eq!(url, "https://example.com/article"),
        _ => panic!("Expected Web action"),
    }
}

#[test]
fn test_auto_detect_stdin_and_file() {
    let stdin_cli = Cli::parse_from(["srz", "-"]);
    assert_eq!(stdin_cli.resolve_action().unwrap(), ResolvedAction::Stdin);

    let stdin_subcmd = Cli::parse_from(["srz", "file", "-"]);
    assert_eq!(
        stdin_subcmd.resolve_action().unwrap(),
        ResolvedAction::Stdin
    );

    // Existing local file (Cargo.toml)
    let file_cli = Cli::parse_from(["srz", "Cargo.toml"]);
    match file_cli.resolve_action().unwrap() {
        ResolvedAction::File { path } => {
            assert_eq!(path, std::path::PathBuf::from("Cargo.toml"));
        }
        _ => panic!("Expected File action for Cargo.toml"),
    }

    let file_subcmd = Cli::parse_from(["srz", "file", "README.md"]);
    match file_subcmd.resolve_action().unwrap() {
        ResolvedAction::File { path } => {
            assert_eq!(path, std::path::PathBuf::from("README.md"));
        }
        _ => panic!("Expected File action for README.md"),
    }
}

#[test]
fn test_auto_detect_book_with_global_flags() {
    let cli = Cli::parse_from([
        "srz",
        "The Great Gatsby",
        "--style",
        "concise",
        "--no-cache",
    ]);

    assert!(cli.no_cache);
    assert_eq!(cli.style, SummaryStyle::Concise);

    match cli.resolve_action().unwrap() {
        ResolvedAction::Book { title } => {
            assert_eq!(title, "The Great Gatsby");
        }
        _ => panic!("Expected Book action"),
    }
}

#[test]
fn test_book_subcommand_flags() {
    let cli = Cli::parse_from([
        "srz",
        "book",
        "Dune",
    ]);

    assert_eq!(cli.max_tokens, 8192);

    match cli.resolve_action().unwrap() {
        ResolvedAction::Book { title } => {
            assert_eq!(title, "Dune");
        }
        _ => panic!("Expected Book action"),
    }
}

#[test]
fn test_terminal_paging_and_plain_flags() {
    use srz::cli::PagingMode;

    let default_cli = Cli::parse_from(["srz", "Dune"]);
    assert_eq!(default_cli.paging, PagingMode::Auto);
    assert!(!default_cli.plain);
    assert_eq!(default_cli.effective_paging(), PagingMode::Auto);

    let always_cli = Cli::parse_from(["srz", "Dune", "--paging", "always"]);
    assert_eq!(always_cli.paging, PagingMode::Always);
    assert_eq!(always_cli.effective_paging(), PagingMode::Always);

    let never_cli = Cli::parse_from(["srz", "Dune", "--paging", "never"]);
    assert_eq!(never_cli.paging, PagingMode::Never);
    assert_eq!(never_cli.effective_paging(), PagingMode::Never);

    let plain_cli = Cli::parse_from(["srz", "Dune", "--plain"]);
    assert!(plain_cli.plain);
}

#[test]
fn test_missing_target_returns_error() {
    let cli = Cli::parse_from(["srz"]);
    assert!(cli.resolve_action().is_err());
}

#[test]
fn test_ask_subcommand_resolution() {
    use srz::cli::is_probable_question;

    assert!(is_probable_question("What is the thesis?"));
    assert!(is_probable_question("How does this compare?"));
    assert!(is_probable_question("Can you explain point 3?"));
    assert!(is_probable_question("Explain the methodology"));
    assert!(is_probable_question("Tell me about the ending?"));
    assert!(!is_probable_question("The Hobbit"));
    assert!(!is_probable_question("https://example.com/article"));
    assert!(!is_probable_question("document.pdf"));

    // Case 1: Target and Question
    let cli1 = Cli::parse_from([
        "srz",
        "ask",
        "https://example.com",
        "What is the key takeaway?",
    ]);
    assert_eq!(
        cli1.resolve_action().unwrap(),
        ResolvedAction::Ask {
            target: Some("https://example.com".to_string()),
            question: Some("What is the key takeaway?".to_string()),
            interactive: false,
            list_chats: false,
            resume: None,
            show_chat: None,
            delete_chat: None,
        }
    );

    // Case 2: Only question provided (auto-detected as question)
    let cli2 = Cli::parse_from(["srz", "ask", "What are the limitations?"]);
    assert_eq!(
        cli2.resolve_action().unwrap(),
        ResolvedAction::Ask {
            target: None,
            question: Some("What are the limitations?".to_string()),
            interactive: false,
            list_chats: false,
            resume: None,
            show_chat: None,
            delete_chat: None,
        }
    );

    // Case 3: Only target provided (e.g. for interactive chat)
    let cli3 = Cli::parse_from(["srz", "ask", "The Hobbit"]);
    assert_eq!(
        cli3.resolve_action().unwrap(),
        ResolvedAction::Ask {
            target: Some("The Hobbit".to_string()),
            question: None,
            interactive: false,
            list_chats: false,
            resume: None,
            show_chat: None,
            delete_chat: None,
        }
    );

    // Case 4: Interactive flag
    let cli4 = Cli::parse_from(["srz", "ask", "--interactive"]);
    assert_eq!(
        cli4.resolve_action().unwrap(),
        ResolvedAction::Ask {
            target: None,
            question: None,
            interactive: true,
            list_chats: false,
            resume: None,
            show_chat: None,
            delete_chat: None,
        }
    );

    // Case 5: Persistent session management flags
    let cli5 = Cli::parse_from(["srz", "ask", "--chats"]);
    assert_eq!(
        cli5.resolve_action().unwrap(),
        ResolvedAction::Ask {
            target: None,
            question: None,
            interactive: false,
            list_chats: true,
            resume: None,
            show_chat: None,
            delete_chat: None,
        }
    );

    let cli6 = Cli::parse_from(["srz", "ask", "--resume", "42"]);
    assert_eq!(
        cli6.resolve_action().unwrap(),
        ResolvedAction::Ask {
            target: None,
            question: None,
            interactive: true,
            list_chats: false,
            resume: Some(42),
            show_chat: None,
            delete_chat: None,
        }
    );
}

#[test]
fn test_browse_subcommand_resolution() {
    let browse_cli = Cli::parse_from(["srz", "browse"]);
    assert_eq!(browse_cli.resolve_action().unwrap(), ResolvedAction::Browse);

    let tui_cli = Cli::parse_from(["srz", "tui"]);
    assert_eq!(tui_cli.resolve_action().unwrap(), ResolvedAction::Browse);

    let library_cli = Cli::parse_from(["srz", "library"]);
    assert_eq!(
        library_cli.resolve_action().unwrap(),
        ResolvedAction::Browse
    );
}

#[test]
fn test_quiet_and_verbose_flags() {
    let quiet_cli = Cli::parse_from(["srz", "Dune", "-q"]);
    assert!(quiet_cli.quiet);
    assert!(!quiet_cli.verbose);

    let verbose_cli = Cli::parse_from(["srz", "web", "https://example.com", "--verbose"]);
    assert!(!verbose_cli.quiet);
    assert!(verbose_cli.verbose);
}

#[test]
fn test_json_flag() {
    let json_cli = Cli::parse_from(["srz", "history", "--json"]);
    assert!(json_cli.json);

    let json_summary_cli = Cli::parse_from(["srz", "Dune", "--json"]);
    assert!(json_summary_cli.json);
}

#[test]
fn test_open_flag() {
    let open_cli = Cli::parse_from(["srz", "The Hobbit", "-b"]);
    assert!(open_cli.open);
    assert_eq!(open_cli.concurrency, 1);

    let long_open_cli = Cli::parse_from(["srz", "The Hobbit", "--open"]);
    assert!(long_open_cli.open);
}

#[test]
fn test_news_subcommand_resolution() {
    let default_news = Cli::parse_from(["srz", "news", "javascript"]);
    assert_eq!(
        default_news.resolve_action().unwrap(),
        ResolvedAction::News {
            topic: "javascript".to_string(),
            timeframe: NewsTimeframe::Day,
            limit: 5,
        }
    );

    let custom_news = Cli::parse_from([
        "srz",
        "news",
        "artificial intelligence",
        "-t",
        "week",
        "-l",
        "8",
    ]);
    assert_eq!(
        custom_news.resolve_action().unwrap(),
        ResolvedAction::News {
            topic: "artificial intelligence".to_string(),
            timeframe: NewsTimeframe::Week,
            limit: 8,
        }
    );
}

#[test]
fn test_news_natural_query_auto_detection() {
    let js_news = Cli::parse_from(["srz", "current javascript news"]);
    assert_eq!(
        js_news.resolve_action().unwrap(),
        ResolvedAction::News {
            topic: "javascript".to_string(),
            timeframe: NewsTimeframe::Day,
            limit: 5,
        }
    );

    let latest_rust = Cli::parse_from(["srz", "latest rust news"]);
    assert_eq!(
        latest_rust.resolve_action().unwrap(),
        ResolvedAction::News {
            topic: "rust".to_string(),
            timeframe: NewsTimeframe::Day,
            limit: 5,
        }
    );

    let news_about_tech = Cli::parse_from(["srz", "news about quantum computing"]);
    assert_eq!(
        news_about_tech.resolve_action().unwrap(),
        ResolvedAction::News {
            topic: "quantum computing".to_string(),
            timeframe: NewsTimeframe::Day,
            limit: 5,
        }
    );

    let general_news = Cli::parse_from(["srz", "current news"]);
    assert_eq!(
        general_news.resolve_action().unwrap(),
        ResolvedAction::News {
            topic: "top stories".to_string(),
            timeframe: NewsTimeframe::Day,
            limit: 5,
        }
    );
}

#[test]
fn test_news_query_helpers() {
    assert!(is_news_query("current javascript news"));
    assert!(is_news_query("latest AI news"));
    assert!(is_news_query("tech news"));
    assert!(is_news_query("news"));
    assert!(is_news_query("current headlines"));
    assert!(!is_news_query("The Great Gatsby"));
    assert!(!is_news_query("Designing Data-Intensive Applications"));

    assert_eq!(clean_news_query("current javascript news"), "javascript");
    assert_eq!(clean_news_query("latest rust news"), "rust");
    assert_eq!(clean_news_query("news about astrophysics"), "astrophysics");
    assert_eq!(clean_news_query("current news"), "top stories");
    assert_eq!(clean_news_query("news"), "top stories");
}

#[test]
fn test_google_news_url_extraction() {
    use srz::news::{extract_article_id, extract_attr_value, parse_batchexecute_destination_url};

    let url = "https://news.google.com/rss/articles/CBMiWmh0dHBzOi8vd3d3LnNpdGVwb2ludC5jb20vamF2YXNjcmlwdC1uZXdzLw?oc=5";
    assert_eq!(
        extract_article_id(url),
        Some("CBMiWmh0dHBzOi8vd3d3LnNpdGVwb2ludC5jb20vamF2YXNjcmlwdC1uZXdzLw")
    );

    let html = r#"<div data-n-a-sg="ABC123xyz" data-n-a-ts="1714200000"></div>"#;
    assert_eq!(extract_attr_value(html, "data-n-a-sg="), Some("ABC123xyz"));
    assert_eq!(extract_attr_value(html, "data-n-a-ts="), Some("1714200000"));

    let rpc_text = ")]}'\n\n[[\"wrb.fr\",\"Fbv4je\",\"[\\\"garturlreq\\\",\\\"https://www.sitepoint.com/javascript-news/\\\"]\"]]";
    assert_eq!(
        parse_batchexecute_destination_url(rpc_text),
        Some("https://www.sitepoint.com/javascript-news/".to_string())
    );
}

#[test]
fn test_stream_flag_and_mode_defaults() {
    use srz::llm::StreamMode;

    // Default: stream is false, stream_mode is Buffered
    let default_cli = Cli::parse_from(["srz", "Dune"]);
    assert!(!default_cli.stream);
    assert!(matches!(
        default_cli.stream_mode("Dune"),
        StreamMode::Buffered
    ));

    // When --stream is provided: stream is true, stream_mode is Stdout
    let stream_cli = Cli::parse_from(["srz", "Dune", "--stream"]);
    assert!(stream_cli.stream);
    match stream_cli.stream_mode("Dune") {
        StreamMode::Stdout { header_title } => assert_eq!(header_title, "Dune"),
        _ => panic!("Expected StreamMode::Stdout"),
    }

    // When --stream is provided but with -o, stream_mode is Buffered
    let file_cli = Cli::parse_from(["srz", "Dune", "--stream", "-o", "out.md"]);
    assert!(file_cli.stream);
    assert!(matches!(file_cli.stream_mode("Dune"), StreamMode::Buffered));

    // When --stream is provided but with --json, stream_mode is Buffered
    let json_cli = Cli::parse_from(["srz", "Dune", "--stream", "--json"]);
    assert!(json_cli.stream);
    assert!(matches!(json_cli.stream_mode("Dune"), StreamMode::Buffered));
}

#[test]
fn test_upgrade_subcommand_flags() {
    // Default srz upgrade
    let upgrade_cli = Cli::parse_from(["srz", "upgrade"]);
    assert_eq!(
        upgrade_cli.resolve_action().unwrap(),
        ResolvedAction::Upgrade {
            check: false,
            force: false,
            version: None,
        }
    );

    // srz upgrade --check
    let check_cli = Cli::parse_from(["srz", "upgrade", "-c"]);
    assert_eq!(
        check_cli.resolve_action().unwrap(),
        ResolvedAction::Upgrade {
            check: true,
            force: false,
            version: None,
        }
    );

    // srz upgrade --force
    let force_cli = Cli::parse_from(["srz", "upgrade", "--force"]);
    assert_eq!(
        force_cli.resolve_action().unwrap(),
        ResolvedAction::Upgrade {
            check: false,
            force: true,
            version: None,
        }
    );

    // srz upgrade --version 0.2.0
    let ver_cli = Cli::parse_from(["srz", "upgrade", "--version", "0.2.0"]);
    assert_eq!(
        ver_cli.resolve_action().unwrap(),
        ResolvedAction::Upgrade {
            check: false,
            force: false,
            version: Some("0.2.0".to_string()),
        }
    );
}

#[test]
fn test_readme_usage_examples_cli_resolution() {
    // 1. Webpage
    let cmd1 = Cli::parse_from([
        "srz",
        "https://en.wikipedia.org/wiki/Rust_(programming_language)",
    ]);
    assert_eq!(
        cmd1.resolve_action().unwrap(),
        ResolvedAction::Web {
            url: "https://en.wikipedia.org/wiki/Rust_(programming_language)"
        }
    );

    // 2. YouTube
    let cmd2 = Cli::parse_from(["srz", "https://youtu.be/5C_HPTJg5ek"]);
    assert_eq!(
        cmd2.resolve_action().unwrap(),
        ResolvedAction::Web {
            url: "https://youtu.be/5C_HPTJg5ek"
        }
    );

    // 3. Local PDF document (auto-detected as file via is_file_like even if missing)
    let cmd3 = Cli::parse_from(["srz", "assets/document.pdf"]);
    assert_eq!(
        cmd3.resolve_action().unwrap(),
        ResolvedAction::File {
            path: std::path::PathBuf::from("assets/document.pdf")
        }
    );
    let cmd3_raw = Cli::parse_from(["srz", "document.pdf"]);
    assert_eq!(
        cmd3_raw.resolve_action().unwrap(),
        ResolvedAction::File {
            path: std::path::PathBuf::from("document.pdf")
        }
    );

    // 4. Book synopsis with bullets and browser open
    let cmd4 = Cli::parse_from([
        "srz",
        "The Rust Programming Language",
        "--style",
        "bullets",
        "--open",
    ]);
    assert_eq!(cmd4.style, SummaryStyle::Bullets);
    assert!(cmd4.open);
    assert_eq!(
        cmd4.resolve_action().unwrap(),
        ResolvedAction::Book {
            title: "The Rust Programming Language",
        }
    );

    // 5. News digest
    let cmd5 = Cli::parse_from(["srz", "news", "Rust language updates", "-t", "week"]);
    assert_eq!(
        cmd5.resolve_action().unwrap(),
        ResolvedAction::News {
            topic: "Rust language updates".to_string(),
            timeframe: NewsTimeframe::Week,
            limit: 5,
        }
    );

    // 6. Unix stdin stream
    let cmd6 = Cli::parse_from(["srz", "-"]);
    assert_eq!(cmd6.resolve_action().unwrap(), ResolvedAction::Stdin);

    // 7. Ask based on latest read
    let cmd7 = Cli::parse_from(["srz", "ask", "What are the main findings?"]);
    assert_eq!(
        cmd7.resolve_action().unwrap(),
        ResolvedAction::Ask {
            target: None,
            question: Some("What are the main findings?".to_string()),
            interactive: false,
            list_chats: false,
            resume: None,
            show_chat: None,
            delete_chat: None,
        }
    );

    // 8. Ask based on specific document
    let cmd8 = Cli::parse_from(["srz", "ask", "assets/document.pdf", "Key takeaways?"]);
    assert_eq!(
        cmd8.resolve_action().unwrap(),
        ResolvedAction::Ask {
            target: Some("assets/document.pdf".to_string()),
            question: Some("Key takeaways?".to_string()),
            interactive: false,
            list_chats: false,
            resume: None,
            show_chat: None,
            delete_chat: None,
        }
    );
    let cmd8_raw = Cli::parse_from(["srz", "ask", "document.pdf", "Key takeaways?"]);
    assert_eq!(
        cmd8_raw.resolve_action().unwrap(),
        ResolvedAction::Ask {
            target: Some("document.pdf".to_string()),
            question: Some("Key takeaways?".to_string()),
            interactive: false,
            list_chats: false,
            resume: None,
            show_chat: None,
            delete_chat: None,
        }
    );

    // 9. Interactive chat
    let cmd9 = Cli::parse_from(["srz", "ask", "-i"]);
    assert_eq!(
        cmd9.resolve_action().unwrap(),
        ResolvedAction::Ask {
            target: None,
            question: None,
            interactive: true,
            list_chats: false,
            resume: None,
            show_chat: None,
            delete_chat: None,
        }
    );

    // 10. TUI browse
    let cmd10 = Cli::parse_from(["srz", "browse"]);
    assert_eq!(cmd10.resolve_action().unwrap(), ResolvedAction::Browse);

    // 11. Find in library
    let cmd11 = Cli::parse_from(["srz", "find", "keyword"]);
    assert_eq!(
        cmd11.resolve_action().unwrap(),
        ResolvedAction::Find { query: "keyword" }
    );

    // 12. History (default limit)
    let cmd12 = Cli::parse_from(["srz", "history"]);
    assert_eq!(
        cmd12.resolve_action().unwrap(),
        ResolvedAction::History { limit: 10 }
    );

    // 13. Stats
    let cmd13 = Cli::parse_from(["srz", "stats"]);
    assert_eq!(cmd13.resolve_action().unwrap(), ResolvedAction::Stats);

    // 14. Cache size
    let cmd14 = Cli::parse_from(["srz", "cache", "size"]);
    assert_eq!(
        cmd14.resolve_action().unwrap(),
        ResolvedAction::Cache(CacheCommand::Size)
    );

    // 15. Cache prune by age
    let cmd15 = Cli::parse_from(["srz", "cache", "prune", "--older-than", "14d"]);
    assert_eq!(
        cmd15.resolve_action().unwrap(),
        ResolvedAction::Cache(CacheCommand::Prune {
            older_than: Some("14d".to_string()),
            max_size: None,
        })
    );

    // 16. Cache prune by max size
    let cmd16 = Cli::parse_from(["srz", "cache", "prune", "--max-size", "500MB"]);
    assert_eq!(
        cmd16.resolve_action().unwrap(),
        ResolvedAction::Cache(CacheCommand::Prune {
            older_than: None,
            max_size: Some("500MB".to_string()),
        })
    );

    // 17. Cache clean
    let cmd17 = Cli::parse_from(["srz", "cache", "clean"]);
    assert_eq!(
        cmd17.resolve_action().unwrap(),
        ResolvedAction::Cache(CacheCommand::Clean { reset_db: false })
    );

    // 18. Cache clean reset db
    let cmd18 = Cli::parse_from(["srz", "cache", "clean", "--reset-db"]);
    assert_eq!(
        cmd18.resolve_action().unwrap(),
        ResolvedAction::Cache(CacheCommand::Clean { reset_db: true })
    );

    // 19. Doctor
    let cmd19 = Cli::parse_from(["srz", "doctor"]);
    assert_eq!(cmd19.resolve_action().unwrap(), ResolvedAction::Doctor);

    // Additional README examples:
    // srz models
    let cmd_models = Cli::parse_from(["srz", "models"]);
    assert_eq!(cmd_models.resolve_action().unwrap(), ResolvedAction::Models);

    // srz config init
    let cmd_cfg_init = Cli::parse_from(["srz", "config", "init"]);
    assert_eq!(
        cmd_cfg_init.resolve_action().unwrap(),
        ResolvedAction::Config(srz::cli::ConfigSubcommand::Init)
    );

    // srz "The Rust Programming Language"
    let cmd_rust_book = Cli::parse_from(["srz", "The Rust Programming Language"]);
    assert_eq!(
        cmd_rust_book.resolve_action().unwrap(),
        ResolvedAction::Book {
            title: "The Rust Programming Language",
        }
    );

    // srz "The Rust Programming Language" --profile openai
    let cmd_rust_prof = Cli::parse_from([
        "srz",
        "The Rust Programming Language",
        "--profile",
        "openai",
    ]);
    assert_eq!(cmd_rust_prof.profile.as_deref(), Some("openai"));

    // srz "1984" --base-url "https://api.openai.com/v1" --model "gpt-4o-mini" --api-key "sk-..."
    let cmd_cloud = Cli::parse_from([
        "srz",
        "1984",
        "--base-url",
        "https://api.openai.com/v1",
        "--model",
        "gpt-4o-mini",
        "--api-key",
        "sk-test",
    ]);
    assert_eq!(cmd_cloud.base_url, "https://api.openai.com/v1");
    assert_eq!(cmd_cloud.model, "gpt-4o-mini");
    assert_eq!(cmd_cloud.api_key, "sk-test");

    // srz index
    let cmd_index = Cli::parse_from(["srz", "index"]);
    assert_eq!(cmd_index.resolve_action().unwrap(), ResolvedAction::Index);
}

#[test]
fn test_file_like_detection_and_document_pdf() {
    use srz::cli::is_file_like;

    assert!(is_file_like("document.pdf"));
    assert!(is_file_like("paper.PDF"));
    assert!(is_file_like("notes.txt"));
    assert!(is_file_like("README.md"));
    assert!(is_file_like("article.html"));
    assert!(is_file_like("./some_file"));
    assert!(is_file_like("../parent_file"));
    assert!(is_file_like("/absolute/path/file"));

    assert!(!is_file_like("The Rust Programming Language"));
    assert!(!is_file_like("The Hobbit"));
    assert!(!is_file_like("news about rust"));
    assert!(!is_file_like("https://example.com"));

    // Test that document.pdf resolves to File even if deleted
    let pdf_cli = Cli::parse_from(["srz", "document.pdf"]);
    match pdf_cli.resolve_action().unwrap() {
        ResolvedAction::File { path } => {
            assert_eq!(path, std::path::PathBuf::from("document.pdf"));
        }
        _ => panic!("Expected File action for document.pdf"),
    }
}
