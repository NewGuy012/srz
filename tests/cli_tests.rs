use clap::Parser;
use srz::cli::{
    clean_news_query, is_news_query, CacheCommand, Cli, NewsTimeframe, ResolvedAction, SummaryStyle,
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
        ResolvedAction::Cache(CacheCommand::Clean)
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
fn test_new_summary_styles() {
    let tldr_cli = Cli::parse_from(["srz", "Dune", "--style", "tldr"]);
    assert_eq!(tldr_cli.style, SummaryStyle::Tldr);
    assert_eq!(tldr_cli.style.as_str(), "tldr");

    let critique_cli = Cli::parse_from(["srz", "Dune", "--style", "critique"]);
    assert_eq!(critique_cli.style, SummaryStyle::Critique);
    assert_eq!(critique_cli.style.as_str(), "critique");

    let eli5_cli = Cli::parse_from(["srz", "Dune", "--style", "eli5"]);
    assert_eq!(eli5_cli.style, SummaryStyle::Eli5);
    assert_eq!(eli5_cli.style.as_str(), "eli5");

    let flashcards_cli = Cli::parse_from(["srz", "Dune", "--style", "flashcards"]);
    assert_eq!(flashcards_cli.style, SummaryStyle::Flashcards);
    assert_eq!(flashcards_cli.style.as_str(), "flashcards");
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
    assert_eq!(stdin_subcmd.resolve_action().unwrap(), ResolvedAction::Stdin);

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
        "--author",
        "F. Scott Fitzgerald",
        "--style",
        "concise",
        "--no-cache",
    ]);

    assert!(cli.no_cache);
    assert_eq!(cli.style, SummaryStyle::Concise);

    match cli.resolve_action().unwrap() {
        ResolvedAction::Book {
            title,
            author,
            no_search,
        } => {
            assert_eq!(title, "The Great Gatsby");
            assert_eq!(author, Some("F. Scott Fitzgerald"));
            assert!(!no_search);
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
        "--author",
        "Frank Herbert",
        "--no-search",
        "--max-tokens",
        "4096",
    ]);

    assert_eq!(cli.max_tokens, 4096);

    match cli.resolve_action().unwrap() {
        ResolvedAction::Book {
            title,
            author,
            no_search,
        } => {
            assert_eq!(title, "Dune");
            assert_eq!(author, Some("Frank Herbert"));
            assert!(no_search);
        }
        _ => panic!("Expected Book action"),
    }
}

#[test]
fn test_missing_target_returns_error() {
    let cli = Cli::parse_from(["srz"]);
    assert!(cli.resolve_action().is_err());
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
fn test_yes_flag() {
    let yes_cli = Cli::parse_from(["srz", "book", "The Hobbit", "-y"]);
    assert!(yes_cli.yes);

    let long_yes_cli = Cli::parse_from(["srz", "The Hobbit", "--yes"]);
    assert!(long_yes_cli.yes);

    let open_cli = Cli::parse_from(["srz", "The Hobbit", "-b"]);
    assert!(open_cli.open);

    let long_open_cli = Cli::parse_from(["srz", "The Hobbit", "--open"]);
    assert!(long_open_cli.open);

    let concurrency_cli = Cli::parse_from(["srz", "The Hobbit", "--concurrency", "4"]);
    assert_eq!(concurrency_cli.concurrency, 4);
}

#[test]
fn test_news_subcommand_resolution() {
    let default_news = Cli::parse_from(["srz", "news", "javascript"]);
    assert_eq!(
        default_news.resolve_action().unwrap(),
        ResolvedAction::News {
            topic: "javascript".to_string(),
            timeframe: NewsTimeframe::Day,
            limit: 4,
        }
    );

    let custom_news = Cli::parse_from([
        "srz", "news", "artificial intelligence", "-t", "week", "-l", "8",
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
            limit: 4,
        }
    );

    let latest_rust = Cli::parse_from(["srz", "latest rust news"]);
    assert_eq!(
        latest_rust.resolve_action().unwrap(),
        ResolvedAction::News {
            topic: "rust".to_string(),
            timeframe: NewsTimeframe::Day,
            limit: 4,
        }
    );

    let news_about_tech = Cli::parse_from(["srz", "news about quantum computing"]);
    assert_eq!(
        news_about_tech.resolve_action().unwrap(),
        ResolvedAction::News {
            topic: "quantum computing".to_string(),
            timeframe: NewsTimeframe::Day,
            limit: 4,
        }
    );

    let general_news = Cli::parse_from(["srz", "current news"]);
    assert_eq!(
        general_news.resolve_action().unwrap(),
        ResolvedAction::News {
            topic: "top stories".to_string(),
            timeframe: NewsTimeframe::Day,
            limit: 4,
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
