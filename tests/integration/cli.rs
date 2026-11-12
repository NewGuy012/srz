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

    let clear_cli = Cli::parse_from(["srz", "cache", "clear"]);
    assert_eq!(
        clear_cli.resolve_action().unwrap(),
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
            shell: srz::cli::Shell::Bash
        }
    );

    let zsh_cli = Cli::parse_from(["srz", "completions", "zsh"]);
    assert_eq!(
        zsh_cli.resolve_action().unwrap(),
        ResolvedAction::Completions {
            shell: srz::cli::Shell::Zsh
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

    // Existing local file (Cargo.toml)
    let file_cli = Cli::parse_from(["srz", "Cargo.toml"]);
    match file_cli.resolve_action().unwrap() {
        ResolvedAction::File { path } => {
            assert_eq!(path, std::path::PathBuf::from("Cargo.toml"));
        }
        _ => panic!("Expected File action for Cargo.toml"),
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
fn test_book_positional_flags() {
    let cli = Cli::parse_from(["srz", "Dune", "--max-tokens", "4096"]);

    assert_eq!(cli.max_tokens, 4096);

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

    let always_cli = Cli::parse_from(["srz", "Dune", "--paging", "always"]);
    assert_eq!(always_cli.paging, PagingMode::Always);

    let never_cli = Cli::parse_from(["srz", "Dune", "--paging", "never"]);
    assert_eq!(never_cli.paging, PagingMode::Never);

    let plain_cli = Cli::parse_from(["srz", "Dune", "--plain"]);
    assert!(plain_cli.plain);
}

#[test]
fn test_missing_target_returns_error() {
    let cli = Cli::parse_from(["srz"]);
    assert!(cli.resolve_action().is_err());
}

#[test]
fn test_cli_syntax_validation_errors() {
    // Unknown flag should fail CLI parsing
    assert!(Cli::try_parse_from(["srz", "--unknown-flag-xyz"]).is_err());

    // Invalid style option should fail CLI parsing
    assert!(Cli::try_parse_from(["srz", "Dune", "--style", "super-long"]).is_err());

    // Invalid paging option should fail CLI parsing
    assert!(Cli::try_parse_from(["srz", "Dune", "--paging", "sometimes"]).is_err());

    // Invalid shell completion name
    assert!(Cli::try_parse_from(["srz", "completions", "unknown_shell"]).is_err());
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

    let verbose_cli = Cli::parse_from(["srz", "https://example.com", "--verbose"]);
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
            timeframe: NewsTimeframe::All,
            limit: 5,
        }
    );

    let stop_words_news = Cli::parse_from(["srz", "news", "Nvidia news"]);
    assert_eq!(
        stop_words_news.resolve_action().unwrap(),
        ResolvedAction::News {
            topic: "Nvidia".to_string(),
            timeframe: NewsTimeframe::All,
            limit: 5,
        }
    );

    let custom_news = Cli::parse_from([
        "srz",
        "news",
        "artificial intelligence",
        "-t",
        "day",
        "-l",
        "8",
    ]);
    assert_eq!(
        custom_news.resolve_action().unwrap(),
        ResolvedAction::News {
            topic: "artificial intelligence".to_string(),
            timeframe: NewsTimeframe::Day,
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
            timeframe: NewsTimeframe::All,
            limit: 5,
        }
    );

    let latest_rust = Cli::parse_from(["srz", "latest rust news"]);
    assert_eq!(
        latest_rust.resolve_action().unwrap(),
        ResolvedAction::News {
            topic: "rust".to_string(),
            timeframe: NewsTimeframe::All,
            limit: 5,
        }
    );

    let news_about_tech = Cli::parse_from(["srz", "news about quantum computing"]);
    assert_eq!(
        news_about_tech.resolve_action().unwrap(),
        ResolvedAction::News {
            topic: "quantum computing".to_string(),
            timeframe: NewsTimeframe::All,
            limit: 5,
        }
    );

    let general_news = Cli::parse_from(["srz", "current news"]);
    assert_eq!(
        general_news.resolve_action().unwrap(),
        ResolvedAction::News {
            topic: "top stories".to_string(),
            timeframe: NewsTimeframe::All,
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
fn test_show_thinking_flag_and_alias() {
    // Default: show_thinking is false
    let default_cli = Cli::parse_from(["srz", "Dune"]);
    assert!(!default_cli.show_thinking);

    // --show-thinking
    let show_cli = Cli::parse_from(["srz", "Dune", "--show-thinking"]);
    assert!(show_cli.show_thinking);

    // alias --thinking
    let alias_cli = Cli::parse_from(["srz", "Dune", "--thinking"]);
    assert!(alias_cli.show_thinking);
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
fn test_cli_action_resolution_scenarios() {
    let web = Cli::parse_from([
        "srz",
        "https://en.wikipedia.org/wiki/Rust_(programming_language)",
    ]);
    assert_eq!(
        web.resolve_action().unwrap(),
        ResolvedAction::Web {
            url: "https://en.wikipedia.org/wiki/Rust_(programming_language)"
        }
    );

    let yt = Cli::parse_from(["srz", "https://youtu.be/5C_HPTJg5ek"]);
    assert_eq!(
        yt.resolve_action().unwrap(),
        ResolvedAction::Web {
            url: "https://youtu.be/5C_HPTJg5ek"
        }
    );

    let news = Cli::parse_from(["srz", "news", "Rust language updates"]);
    assert_eq!(
        news.resolve_action().unwrap(),
        ResolvedAction::News {
            topic: "Rust language updates".to_string(),
            timeframe: NewsTimeframe::All,
            limit: 5,
        }
    );

    for path in ["assets/document.pdf", "document.pdf"] {
        let cli = Cli::parse_from(["srz", path]);
        assert_eq!(
            cli.resolve_action().unwrap(),
            ResolvedAction::File {
                path: std::path::PathBuf::from(path)
            }
        );
    }

    assert_eq!(
        Cli::parse_from(["srz", "-"]).resolve_action().unwrap(),
        ResolvedAction::Stdin
    );

    let book = Cli::parse_from([
        "srz",
        "The Rust Programming Language",
        "--style",
        "bullets",
        "--open",
        "--profile",
        "openai",
    ]);
    assert_eq!(book.style, SummaryStyle::Bullets);
    assert!(book.open);
    assert_eq!(book.profile.as_deref(), Some("openai"));
    assert_eq!(
        book.resolve_action().unwrap(),
        ResolvedAction::Book {
            title: "The Rust Programming Language"
        }
    );

    let ask_file = Cli::parse_from(["srz", "ask", "document.pdf", "Key takeaways?"]);
    assert_eq!(
        ask_file.resolve_action().unwrap(),
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

    let simple_cases: &[(&[&str], ResolvedAction<'_>)] = &[
        (&["srz", "browse"], ResolvedAction::Browse),
        (
            &["srz", "find", "keyword"],
            ResolvedAction::Find { query: "keyword" },
        ),
        (&["srz", "history"], ResolvedAction::History { limit: 10 }),
        (&["srz", "stats"], ResolvedAction::Stats),
        (
            &["srz", "cache", "size"],
            ResolvedAction::Cache(CacheCommand::Size),
        ),
        (&["srz", "doctor"], ResolvedAction::Doctor),
        (&["srz", "recommend"], ResolvedAction::Recommend),
        (&["srz", "models"], ResolvedAction::Models),
        (
            &["srz", "config", "init"],
            ResolvedAction::Config(srz::cli::ConfigSubcommand::Init),
        ),
        (&["srz", "index"], ResolvedAction::Index),
    ];
    for (argv, expected) in simple_cases {
        let cli = Cli::parse_from(*argv);
        assert_eq!(&cli.resolve_action().unwrap(), expected);
    }
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

#[test]
fn test_feed_url_detection_and_routing() {
    use srz::cli::is_feed_url;

    assert!(is_feed_url("https://blog.rust-lang.org/feed.xml"));
    assert!(is_feed_url("https://example.com/rss.xml"));
    assert!(is_feed_url("https://example.com/atom.xml"));
    assert!(is_feed_url("https://example.com/feed"));
    assert!(is_feed_url("https://example.com/rss"));
    assert!(is_feed_url("https://example.com/feed.json"));
    assert!(is_feed_url("https://news.google.com/rss/search?q=rust"));
    assert!(is_feed_url("https://example.com/articles?format=rss"));

    assert!(!is_feed_url("https://example.com/article"));
    assert!(!is_feed_url(
        "https://en.wikipedia.org/wiki/Rust_(programming_language)"
    ));

    let feed_cli = Cli::parse_from(["srz", "https://blog.rust-lang.org/feed.xml"]);
    assert_eq!(
        feed_cli.resolve_action().unwrap(),
        ResolvedAction::News {
            topic: "https://blog.rust-lang.org/feed.xml".to_string(),
            timeframe: NewsTimeframe::All,
            limit: 5,
        }
    );
}

#[test]
fn test_recommend_simplified_naming_for_llama_gui() {
    use srz::system::hardware::{HardwareProfile, HardwareTier};

    // Verify default CLI model uses verbose full model name for backend compatibility
    let default_cli = Cli::parse_from(["srz", "https://example.com"]);
    assert_eq!(default_cli.model, "ggml-org/gemma-4-E4B-it-GGUF:Q8_0");

    // Tier 8GB
    let p8 = HardwareProfile::from_specs(8 * 1024 * 1024 * 1024, "Apple M1".to_string());
    assert_eq!(p8.tier, HardwareTier::Tier8GB);
    assert_eq!(p8.recommended_model, "ggml-org/gemma-4-E4B-it-GGUF:Q8_0");
    assert_eq!(p8.concise_model, "gemma-4");
    assert_eq!(p8.recommended_size, "E4B");
    assert_eq!(p8.recommended_quant, "Q8_0");
    assert_eq!(p8.disk_size, "~8.6 GB");
    assert_eq!(p8.ram_footprint, "~8.9 GB");
    assert_eq!(p8.recommended_context, 8192);
    assert_eq!(p8.recommended_context_display, "8k ctx");
    assert_eq!(
        p8.llama_gui_selection(),
        "gemma-4  |  E4B  |  Q8_0  |  8k ctx"
    );

    // Tier 16GB
    let p16 = HardwareProfile::from_specs(16 * 1024 * 1024 * 1024, "Apple M2".to_string());
    assert_eq!(p16.tier, HardwareTier::Tier16GB);
    assert_eq!(p16.recommended_model, "ggml-org/gemma-4-E4B-it-GGUF:Q8_0");
    assert_eq!(p16.concise_model, "gemma-4");
    assert_eq!(p16.recommended_size, "E4B");
    assert_eq!(p16.recommended_quant, "Q8_0");
    assert_eq!(p16.disk_size, "~8.6 GB");
    assert_eq!(p16.ram_footprint, "~9.4 GB");
    assert_eq!(p16.recommended_context, 32768);
    assert_eq!(p16.recommended_context_display, "32k ctx");
    assert_eq!(
        p16.llama_gui_selection(),
        "gemma-4  |  E4B  |  Q8_0  |  32k ctx"
    );

    // Tier 32GB
    let p32 = HardwareProfile::from_specs(32 * 1024 * 1024 * 1024, "Apple M3 Pro".to_string());
    assert_eq!(p32.tier, HardwareTier::Tier32GB);
    assert_eq!(p32.recommended_model, "ggml-org/gemma-4-E4B-it-GGUF:Q8_0");
    assert_eq!(p32.concise_model, "gemma-4");
    assert_eq!(p32.recommended_size, "E4B");
    assert_eq!(p32.recommended_quant, "Q8_0");
    assert_eq!(p32.disk_size, "~8.6 GB");
    assert_eq!(p32.ram_footprint, "~10 GB");
    assert_eq!(p32.recommended_context, 65536);
    assert_eq!(p32.recommended_context_display, "64k ctx");
    assert_eq!(
        p32.llama_gui_selection(),
        "gemma-4  |  E4B  |  Q8_0  |  64k ctx"
    );

    // Tier 64GB+
    let p64 = HardwareProfile::from_specs(64 * 1024 * 1024 * 1024, "Apple M3 Max".to_string());
    assert_eq!(p64.tier, HardwareTier::Tier64GBPlus);
    assert_eq!(
        p64.recommended_model,
        "ggml-org/gemma-4-26B-A4B-it-GGUF:Q8_0"
    );
    assert_eq!(p64.concise_model, "gemma-4");
    assert_eq!(p64.recommended_size, "26B-A4B");
    assert_eq!(p64.recommended_quant, "Q8_0");
    assert_eq!(p64.disk_size, "~28.0 GB");
    assert_eq!(p64.ram_footprint, "~30–32 GB");
    assert_eq!(p64.recommended_context, 131072);
    assert_eq!(p64.recommended_context_display, "128k ctx");
    assert_eq!(
        p64.llama_gui_selection(),
        "gemma-4  |  26B-A4B  |  Q8_0  |  128k ctx"
    );

    // Verify template config generation matches
    let template = srz::config::Config::template_for_hardware(&p16);
    assert!(template.contains("model = \"ggml-org/gemma-4-E4B-it-GGUF:Q8_0\""));
    assert!(template.contains("context_window = 32768"));
}

#[test]
fn test_recommend_subcommand_execution() {
    assert!(srz::commands::handle_recommend(false).is_ok());
    assert!(srz::commands::handle_recommend(true).is_ok());
}

#[tokio::test]
async fn test_doctor_subcommand_execution() {
    let cli = Cli::parse_from(["srz", "doctor"]);
    let config = srz::config::Config::default();
    let temp_cache_dir = srz::fs_util::tempdir().unwrap();
    let cache = srz::storage::cache::Cache::with_dir(temp_cache_dir.path().to_path_buf(), true);

    assert!(
        srz::commands::handle_doctor(&cli, &config, None, &cache, false)
            .await
            .is_ok()
    );
    assert!(
        srz::commands::handle_doctor(&cli, &config, None, &cache, true)
            .await
            .is_ok()
    );
}

#[test]
fn test_local_model_not_detected_error_formatting() {
    let raw_resp = r#"{"error":{"code":400,"message":"model 'gemma-4' not found","type":"invalid_request_error"}}"#;
    let err = srz::llm::LlmClient::format_http_error(
        reqwest::StatusCode::BAD_REQUEST,
        raw_resp,
        Some("model 'gemma-4' not found"),
        "gemma-4",
        "http://localhost:9931/v1",
        None,
    );
    assert!(err.contains("Local model not detected: model 'gemma-4' was not found"));
    assert!(err.contains("http://localhost:9931/v1"));
}

#[test]
fn test_suggest_command_typo_detection() {
    use srz::cli::suggest_command;

    assert_eq!(suggest_command("doctur"), Some("doctor"));
    assert_eq!(suggest_command("docotr"), Some("doctor"));
    assert_eq!(suggest_command("docter"), Some("doctor"));
    assert_eq!(suggest_command("Doctor"), Some("doctor"));
    assert_eq!(suggest_command("DOCTOR"), Some("doctor"));
    assert_eq!(suggest_command("histry"), Some("history"));
    assert_eq!(suggest_command("histor"), Some("history"));
    assert_eq!(suggest_command("stat"), Some("stats"));
    assert_eq!(suggest_command("model"), Some("models"));
    assert_eq!(suggest_command("recommed"), Some("recommend"));
    assert_eq!(suggest_command("upgrde"), Some("upgrade"));
    assert_eq!(suggest_command("cach"), Some("cache"));
    assert_eq!(suggest_command("confg"), Some("config"));
    assert_eq!(suggest_command("tu1"), Some("tui"));
    assert_eq!(suggest_command("librery"), Some("library"));
    assert_eq!(suggest_command("exprot"), Some("export"));
}

#[test]
fn test_suggest_command_no_false_positives() {
    use srz::cli::suggest_command;

    assert_eq!(suggest_command("Dune"), None);
    assert_eq!(suggest_command("1984"), None);
    assert_eq!(suggest_command("Hamlet"), None);
    assert_eq!(suggest_command("Macbeth"), None);
    assert_eq!(suggest_command("Doctor Zhivago"), None);
    assert_eq!(suggest_command("The Great Gatsby"), None);
    assert_eq!(suggest_command("Cat"), None);
    assert_eq!(suggest_command("War"), None);
    assert_eq!(suggest_command("It"), None);
    assert_eq!(suggest_command(""), None);
}

#[test]
fn test_resolve_action_command_typo_error() {
    let cli = Cli::parse_from(["srz", "doctur"]);
    let err = cli.resolve_action().unwrap_err();
    assert!(err.contains("Unrecognized command 'doctur'"));
    assert!(err.contains("tip: a similar command exists: 'doctor'"));
}

#[test]
fn test_export_cli_resolution() {
    let export_cli = Cli::parse_from(["srz", "export", "-f", "json", "-o", "/tmp/out.json"]);
    assert_eq!(
        export_cli.resolve_action().unwrap(),
        ResolvedAction::Export {
            format: srz::cli::ExportFormat::Json,
            output: std::path::PathBuf::from("/tmp/out.json"),
        }
    );
}
