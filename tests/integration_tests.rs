use clap::{CommandFactory, FromArgMatches, Parser};
use srz::cli::{
    CacheCommand, Cli, ConfigSubcommand, ExportFormat, NewsTimeframe, PagingMode, ResolvedAction,
    Shell, SummaryStyle, clean_news_query, is_feed_url, is_file_like, is_news_query,
    is_probable_question, suggest_command,
};
use srz::config::{Config, ProfileConfig};
use srz::llm::StreamMode;
use std::path::PathBuf;
use std::time::Duration;

fn news_action(topic: &str, timeframe: NewsTimeframe, limit: usize) -> ResolvedAction<'static> {
    ResolvedAction::News {
        topic: topic.to_string(),
        timeframe,
        limit,
    }
}

fn ask_action(
    target: Option<&str>,
    question: Option<&str>,
    interactive: bool,
    list_chats: bool,
    resume: Option<i64>,
) -> ResolvedAction<'static> {
    ResolvedAction::Ask {
        target: target.map(str::to_string),
        question: question.map(str::to_string),
        interactive,
        list_chats,
        resume,
        show_chat: None,
        delete_chat: None,
    }
}

#[test]
fn test_cli_action_resolution_and_validation() {
    let valid_cases: &[(&[&str], ResolvedAction<'_>)] = &[
        // Cache subcommands
        (
            &["srz", "cache", "dir"],
            ResolvedAction::Cache(CacheCommand::Dir),
        ),
        (
            &["srz", "cache", "size"],
            ResolvedAction::Cache(CacheCommand::Size),
        ),
        (
            &["srz", "cache", "clean"],
            ResolvedAction::Cache(CacheCommand::Clean { reset_db: false }),
        ),
        (
            &["srz", "cache", "clear"],
            ResolvedAction::Cache(CacheCommand::Clean { reset_db: false }),
        ),
        (
            &["srz", "cache", "clean", "--reset-db"],
            ResolvedAction::Cache(CacheCommand::Clean { reset_db: true }),
        ),
        (
            &[
                "srz",
                "cache",
                "prune",
                "--older-than",
                "14d",
                "--max-size",
                "500MB",
            ],
            ResolvedAction::Cache(CacheCommand::Prune {
                older_than: Some("14d".to_string()),
                max_size: Some("500MB".to_string()),
            }),
        ),
        // Core & library subcommands
        (&["srz", "history"], ResolvedAction::History { limit: 10 }),
        (
            &["srz", "history", "--limit", "20"],
            ResolvedAction::History { limit: 20 },
        ),
        (&["srz", "stats"], ResolvedAction::Stats),
        (
            &["srz", "find", "Melange"],
            ResolvedAction::Find { query: "Melange" },
        ),
        (&["srz", "models"], ResolvedAction::Models),
        (&["srz", "doctor"], ResolvedAction::Doctor),
        (&["srz", "recommend"], ResolvedAction::Recommend),
        (&["srz", "index"], ResolvedAction::Index),
        (&["srz", "browse"], ResolvedAction::Browse),
        (&["srz", "tui"], ResolvedAction::Browse),
        (&["srz", "library"], ResolvedAction::Browse),
        (
            &["srz", "completions", "bash"],
            ResolvedAction::Completions { shell: Shell::Bash },
        ),
        (
            &["srz", "completions", "zsh"],
            ResolvedAction::Completions { shell: Shell::Zsh },
        ),
        (
            &["srz", "config", "path"],
            ResolvedAction::Config(ConfigSubcommand::Path),
        ),
        (
            &["srz", "config", "show"],
            ResolvedAction::Config(ConfigSubcommand::Show),
        ),
        (
            &["srz", "config", "init"],
            ResolvedAction::Config(ConfigSubcommand::Init),
        ),
        (
            &["srz", "export", "-f", "json", "-o", "/tmp/out.json"],
            ResolvedAction::Export {
                format: ExportFormat::Json,
                output: PathBuf::from("/tmp/out.json"),
            },
        ),
        // Upgrade flags
        (
            &["srz", "upgrade"],
            ResolvedAction::Upgrade {
                check: false,
                force: false,
                version: None,
            },
        ),
        (
            &["srz", "upgrade", "-c"],
            ResolvedAction::Upgrade {
                check: true,
                force: false,
                version: None,
            },
        ),
        (
            &["srz", "upgrade", "--force"],
            ResolvedAction::Upgrade {
                check: false,
                force: true,
                version: None,
            },
        ),
        (
            &["srz", "upgrade", "--version", "0.2.0"],
            ResolvedAction::Upgrade {
                check: false,
                force: false,
                version: Some("0.2.0".to_string()),
            },
        ),
        // Ask subcommand
        (
            &[
                "srz",
                "ask",
                "https://example.com",
                "What is the key takeaway?",
            ],
            ask_action(
                Some("https://example.com"),
                Some("What is the key takeaway?"),
                false,
                false,
                None,
            ),
        ),
        (
            &["srz", "ask", "document.pdf", "Key takeaways?"],
            ask_action(
                Some("document.pdf"),
                Some("Key takeaways?"),
                false,
                false,
                None,
            ),
        ),
        (
            &["srz", "ask", "What are the limitations?"],
            ask_action(None, Some("What are the limitations?"), false, false, None),
        ),
        (
            &["srz", "ask", "The Hobbit"],
            ask_action(Some("The Hobbit"), None, false, false, None),
        ),
        (
            &["srz", "ask", "--interactive"],
            ask_action(None, None, true, false, None),
        ),
        (
            &["srz", "ask", "--chats"],
            ask_action(None, None, false, true, None),
        ),
        (
            &["srz", "ask", "--resume", "42"],
            ask_action(None, None, true, false, Some(42)),
        ),
        // News subcommand & natural query detection
        (
            &["srz", "news", "javascript"],
            news_action("javascript", NewsTimeframe::All, 5),
        ),
        (
            &["srz", "news", "Nvidia news"],
            news_action("Nvidia", NewsTimeframe::All, 5),
        ),
        (
            &[
                "srz",
                "news",
                "artificial intelligence",
                "-t",
                "day",
                "-l",
                "8",
            ],
            news_action("artificial intelligence", NewsTimeframe::Day, 8),
        ),
        (
            &["srz", "news", "Rust language updates"],
            news_action("Rust language updates", NewsTimeframe::All, 5),
        ),
        (
            &["srz", "current javascript news"],
            news_action("javascript", NewsTimeframe::All, 5),
        ),
        (
            &["srz", "latest rust news"],
            news_action("rust", NewsTimeframe::All, 5),
        ),
        (
            &["srz", "news about quantum computing"],
            news_action("quantum computing", NewsTimeframe::All, 5),
        ),
        (
            &["srz", "current news"],
            news_action("top stories", NewsTimeframe::All, 5),
        ),
        (
            &["srz", "https://blog.rust-lang.org/feed.xml"],
            news_action("https://blog.rust-lang.org/feed.xml", NewsTimeframe::All, 5),
        ),
        // Target auto-detection (URL, stdin, local/missing file, book)
        (
            &["srz", "https://example.com/article"],
            ResolvedAction::Web {
                url: "https://example.com/article",
            },
        ),
        (
            &[
                "srz",
                "https://en.wikipedia.org/wiki/Rust_(programming_language)",
            ],
            ResolvedAction::Web {
                url: "https://en.wikipedia.org/wiki/Rust_(programming_language)",
            },
        ),
        (
            &["srz", "https://youtu.be/5C_HPTJg5ek"],
            ResolvedAction::Web {
                url: "https://youtu.be/5C_HPTJg5ek",
            },
        ),
        (&["srz", "-"], ResolvedAction::Stdin),
        (
            &["srz", "Cargo.toml"],
            ResolvedAction::File {
                path: PathBuf::from("Cargo.toml"),
            },
        ),
        (
            &["srz", "assets/document.pdf"],
            ResolvedAction::File {
                path: PathBuf::from("assets/document.pdf"),
            },
        ),
        (
            &["srz", "document.pdf"],
            ResolvedAction::File {
                path: PathBuf::from("document.pdf"),
            },
        ),
        (
            &[
                "srz",
                "The Great Gatsby",
                "--style",
                "concise",
                "--no-cache",
            ],
            ResolvedAction::Book {
                title: "The Great Gatsby",
            },
        ),
        (
            &["srz", "Dune", "--max-tokens", "4096"],
            ResolvedAction::Book { title: "Dune" },
        ),
        (
            &[
                "srz",
                "The Rust Programming Language",
                "--style",
                "bullets",
                "--open",
                "--profile",
                "openai",
            ],
            ResolvedAction::Book {
                title: "The Rust Programming Language",
            },
        ),
    ];

    for (argv, expected) in valid_cases {
        let cli = Cli::parse_from(*argv);
        assert_eq!(&cli.resolve_action().unwrap(), expected, "argv: {argv:?}");
    }

    assert!(Cli::parse_from(["srz"]).resolve_action().is_err());

    let typo_err = Cli::parse_from(["srz", "doctur"])
        .resolve_action()
        .unwrap_err();
    assert!(typo_err.contains("Unrecognized command 'doctur'"));
    assert!(typo_err.contains("tip: a similar command exists: 'doctor'"));

    for bad_argv in [
        &["srz", "--unknown-flag-xyz"][..],
        &["srz", "Dune", "--style", "super-long"][..],
        &["srz", "Dune", "--paging", "sometimes"][..],
        &["srz", "completions", "unknown_shell"][..],
    ] {
        assert!(
            Cli::try_parse_from(bad_argv).is_err(),
            "bad_argv: {bad_argv:?}"
        );
    }
}

#[test]
fn test_cli_global_flags_and_stream_mode() {
    let default_cli = Cli::parse_from(["srz", "Dune"]);
    assert_eq!(default_cli.style, SummaryStyle::Concise);
    assert_eq!(default_cli.model, "ggml-org/gemma-4-E4B-it-GGUF:Q8_0");
    assert_eq!(default_cli.paging, PagingMode::Auto);
    assert!(!default_cli.plain);
    assert!(!default_cli.stream);
    assert!(!default_cli.show_thinking);
    assert!(matches!(
        default_cli.stream_mode("Dune"),
        StreamMode::Buffered
    ));

    for (arg, expected, as_str) in [
        ("bullets", SummaryStyle::Bullets, "bullets"),
        ("detailed", SummaryStyle::Detailed, "detailed"),
    ] {
        let c = Cli::parse_from(["srz", "Dune", "--style", arg]);
        assert_eq!(c.style, expected);
        assert_eq!(c.style.as_str(), as_str);
    }

    assert_eq!(
        Cli::parse_from(["srz", "Dune", "--paging", "always"]).paging,
        PagingMode::Always
    );
    assert_eq!(
        Cli::parse_from(["srz", "Dune", "--paging", "never"]).paging,
        PagingMode::Never
    );
    assert!(Cli::parse_from(["srz", "Dune", "--plain"]).plain);
    assert!(Cli::parse_from(["srz", "Dune", "--no-cache"]).no_cache);
    assert_eq!(
        Cli::parse_from(["srz", "Dune", "--max-tokens", "4096"]).max_tokens,
        4096
    );
    assert!(Cli::parse_from(["srz", "history", "--json"]).json);
    assert!(Cli::parse_from(["srz", "Dune", "--json"]).json);
    assert!(Cli::parse_from(["srz", "The Hobbit", "-b"]).open);
    assert!(Cli::parse_from(["srz", "The Hobbit", "--open"]).open);
    assert!(Cli::parse_from(["srz", "Dune", "--show-thinking"]).show_thinking);
    assert!(Cli::parse_from(["srz", "Dune", "--thinking"]).show_thinking);

    let quiet = Cli::parse_from(["srz", "Dune", "-q"]);
    assert!(quiet.quiet && !quiet.verbose);
    let verbose = Cli::parse_from(["srz", "https://example.com", "--verbose"]);
    assert!(!verbose.quiet && verbose.verbose);

    let profiled = Cli::parse_from(["srz", "Book", "--profile", "openai"]);
    assert_eq!(profiled.profile.as_deref(), Some("openai"));

    let stream_cli = Cli::parse_from(["srz", "Dune", "--stream"]);
    assert!(stream_cli.stream);
    assert!(matches!(
        stream_cli.stream_mode("Dune"),
        StreamMode::Stdout { header_title } if header_title == "Dune"
    ));
    for buffered_args in [
        &["srz", "Dune", "--stream", "-o", "out.md"][..],
        &["srz", "Dune", "--stream", "--json"][..],
    ] {
        let c = Cli::parse_from(buffered_args);
        assert!(c.stream);
        assert!(matches!(c.stream_mode("Dune"), StreamMode::Buffered));
    }
}

#[test]
fn test_cli_detection_and_typo_helpers() {
    for q in [
        "What is the thesis?",
        "How does this compare?",
        "Can you explain point 3?",
        "Explain the methodology",
        "Tell me about the ending?",
    ] {
        assert!(is_probable_question(q), "{q}");
    }
    for non_q in ["The Hobbit", "https://example.com/article", "document.pdf"] {
        assert!(!is_probable_question(non_q), "{non_q}");
    }

    for file_like in [
        "document.pdf",
        "paper.PDF",
        "notes.txt",
        "README.md",
        "article.html",
        "./some_file",
        "../parent_file",
        "/absolute/path/file",
    ] {
        assert!(is_file_like(file_like), "{file_like}");
    }
    for not_file in [
        "The Rust Programming Language",
        "The Hobbit",
        "news about rust",
        "https://example.com",
    ] {
        assert!(!is_file_like(not_file), "{not_file}");
    }

    for feed in [
        "https://blog.rust-lang.org/feed.xml",
        "https://example.com/rss.xml",
        "https://example.com/atom.xml",
        "https://example.com/feed",
        "https://example.com/rss",
        "https://example.com/feed.json",
        "https://news.google.com/rss/search?q=rust",
        "https://example.com/articles?format=rss",
    ] {
        assert!(is_feed_url(feed), "{feed}");
    }
    for not_feed in [
        "https://example.com/article",
        "https://en.wikipedia.org/wiki/Rust_(programming_language)",
    ] {
        assert!(!is_feed_url(not_feed), "{not_feed}");
    }

    for q in [
        "current javascript news",
        "latest AI news",
        "tech news",
        "news",
        "current headlines",
    ] {
        assert!(is_news_query(q), "{q}");
    }
    for non_news in ["The Great Gatsby", "Designing Data-Intensive Applications"] {
        assert!(!is_news_query(non_news), "{non_news}");
    }
    for (raw, cleaned) in [
        ("current javascript news", "javascript"),
        ("latest rust news", "rust"),
        ("news about astrophysics", "astrophysics"),
        ("current news", "top stories"),
        ("news", "top stories"),
    ] {
        assert_eq!(clean_news_query(raw), cleaned);
    }

    for (typo, expected) in [
        ("doctur", "doctor"),
        ("docotr", "doctor"),
        ("docter", "doctor"),
        ("Doctor", "doctor"),
        ("DOCTOR", "doctor"),
        ("histry", "history"),
        ("histor", "history"),
        ("stat", "stats"),
        ("model", "models"),
        ("recommed", "recommend"),
        ("upgrde", "upgrade"),
        ("cach", "cache"),
        ("confg", "config"),
        ("tu1", "tui"),
        ("librery", "library"),
        ("exprot", "export"),
    ] {
        assert_eq!(suggest_command(typo), Some(expected), "{typo}");
    }
    for title in [
        "Dune",
        "1984",
        "Hamlet",
        "Macbeth",
        "Doctor Zhivago",
        "The Great Gatsby",
        "Cat",
        "War",
        "It",
        "",
    ] {
        assert_eq!(suggest_command(title), None, "{title}");
    }
}

#[test]
fn test_config_toml_deserialization_and_cli_merge() {
    let parse_with_config = |argv: &[&str], config: &Config| {
        let matches = Cli::command().get_matches_from(argv);
        let mut args = Cli::from_arg_matches(&matches).unwrap();
        args.apply_config(config, Some(&matches));
        args
    };

    let toml_str = r#"
base_url = "http://localhost:11434/v1"
model = "llama3:8b"
style = "bullets"
max_tokens = 4096
context_window = 32768

[cache]
max_size_mb = 750
pages_ttl_days = 3
search_ttl_days = 10
summaries_ttl_days = 120
views_ttl_days = 5
auto_prune = false

[profiles.fast]
model = "qwen2.5:3b"
base_url = "http://localhost:9931/v1"
stream = true

[profiles.deep]
model = "deepseek-r1:14b"
style = "detailed"
context_window = 65536
"#;

    let config: Config = toml::from_str(toml_str).unwrap();
    assert_eq!(
        config.base_url.as_deref(),
        Some("http://localhost:11434/v1")
    );
    assert_eq!(config.model.as_deref(), Some("llama3:8b"));
    assert_eq!(config.style, Some(SummaryStyle::Bullets));
    assert_eq!(config.max_tokens, Some(4096));
    assert_eq!(config.context_window, Some(32768));

    let fast = config.profiles.get("fast").unwrap();
    assert_eq!(fast.model.as_deref(), Some("qwen2.5:3b"));
    assert_eq!(fast.base_url.as_deref(), Some("http://localhost:9931/v1"));
    assert_eq!(fast.stream, Some(true));

    let deep = config.profiles.get("deep").unwrap();
    assert_eq!(deep.model.as_deref(), Some("deepseek-r1:14b"));
    assert_eq!(deep.style, Some(SummaryStyle::Detailed));
    assert_eq!(deep.context_window, Some(65536));

    let cache_cfg = config
        .cache
        .as_ref()
        .expect("Cache config should be present");
    assert_eq!(cache_cfg.max_size_mb, Some(750));
    let policy = cache_cfg.to_policy();
    assert_eq!(policy.max_bytes, 750 * 1024 * 1024);
    assert_eq!(policy.pages_ttl, Duration::from_secs(3 * 86400));
    assert_eq!(policy.search_ttl, Duration::from_secs(10 * 86400));
    assert_eq!(policy.summaries_ttl, Duration::from_secs(120 * 86400));
    assert_eq!(policy.views_ttl, Duration::from_secs(5 * 86400));
    assert_eq!(policy.auto_prune_interval, Duration::MAX);

    // Config populates unsupplied CLI args
    let synthetic = Config {
        settings: ProfileConfig {
            base_url: Some("http://ollama:11434/v1".to_string()),
            model: Some("custom-model:latest".to_string()),
            style: Some(SummaryStyle::Detailed),
            max_tokens: Some(2048),
            stream: Some(true),
            ..Default::default()
        },
        ..Default::default()
    };
    let applied = parse_with_config(&["srz", "The Hobbit"], &synthetic);
    assert_eq!(applied.base_url, "http://ollama:11434/v1");
    assert_eq!(applied.model, "custom-model:latest");
    assert_eq!(applied.style, SummaryStyle::Detailed);
    assert_eq!(applied.max_tokens, 2048);
    assert!(applied.stream);

    // CLI flags take precedence over root config; profile overrides root; CLI overrides profile
    let cli_override = parse_with_config(
        &[
            "srz",
            "The Hobbit",
            "--base-url",
            "http://cli-url/v1",
            "--style",
            "concise",
        ],
        &config,
    );
    assert_eq!(cli_override.base_url, "http://cli-url/v1");
    assert_eq!(cli_override.style, SummaryStyle::Concise);
    assert_eq!(cli_override.model, "llama3:8b");

    let profiled = parse_with_config(&["srz", "The Hobbit", "--profile", "deep"], &config);
    assert_eq!(profiled.model, "deepseek-r1:14b");
    assert_eq!(profiled.style, SummaryStyle::Detailed);
    assert_eq!(profiled.context_window, Some(65536));

    let ctx_override =
        parse_with_config(&["srz", "The Hobbit", "--context-window", "16384"], &config);
    assert_eq!(ctx_override.context_window, Some(16384));

    let p16 = srz::system::hardware::HardwareProfile::from_specs(
        16 * 1024 * 1024 * 1024,
        "Apple M2".to_string(),
    );
    let template = Config::template_for_hardware(&p16);
    assert!(template.contains("model = \"ggml-org/gemma-4-E4B-it-GGUF:Q8_0\""));
    assert!(template.contains("context_window = 32768"));
}

#[test]
fn test_cross_module_clients_and_assets_pdf() {
    let scraper = srz::scraper::WebScraper::new().unwrap();
    let searcher = srz::search::BookSearcher::with_client(scraper.client().clone());
    let news = srz::news::GoogleNewsClient::with_client(scraper.client().clone());
    for client in [scraper.client(), searcher.client(), news.client()] {
        assert!(client.get("https://example.com").build().is_ok());
    }

    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/document.pdf");
    assert!(
        path.exists(),
        "assets/document.pdf must exist for CLI access and tests"
    );
    let page = srz::sources::document::PdfExtractor::extract_file(&path)
        .expect("Failed to extract assets/document.pdf");
    for expected in [
        "THE HISTORY OF RUST",
        "Ownership & Borrowing",
        "Graydon Hoare",
        "The Rust Foundation",
    ] {
        assert!(page.content.contains(expected));
    }
    assert!(page.word_count >= 100);

    let doc = srz::sources::DocumentLoader::load_file(&path, &scraper)
        .expect("CLI DocumentLoader failed to access assets/document.pdf");
    assert!(doc.word_count >= 100);
    assert!(doc.content.contains("THE HISTORY OF RUST"));
    assert!(doc.content.contains("CHRONOLOGICAL MILESTONE TIMELINE"));
}

#[tokio::test]
async fn test_commands_recommend_and_doctor_execution() {
    struct TempTestDir(PathBuf);
    impl Drop for TempTestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    assert!(srz::commands::handle_recommend(false).is_ok());
    assert!(srz::commands::handle_recommend(true).is_ok());

    let cli = Cli::parse_from(["srz", "doctor"]);
    let config = Config::default();
    let dir_path = std::env::temp_dir().join(format!("srz-doctor-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir_path).unwrap();
    let temp_cache_dir = TempTestDir(dir_path);
    let cache = srz::storage::cache::Cache::with_dir(temp_cache_dir.0.clone(), true);

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
