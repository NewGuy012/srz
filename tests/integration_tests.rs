use clap::{CommandFactory, FromArgMatches, Parser};
use srz::config::{
    AskSource, CacheCommand, Cli, Commands, Config, ExportFormat, NewsTimeframe, PagingMode,
    ProfileConfig, ResolvedAction, SummaryStyle,
};
use srz::engine::llm::StreamMode;
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
    source: AskSource,
    question: Option<&str>,
    interactive: bool,
    list_chats: bool,
    resume: Option<i64>,
) -> ResolvedAction<'static> {
    ResolvedAction::Ask {
        source,
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
    for (argv, expected) in [
        (
            &["srz", "web", "https://example.com/article"][..],
            ResolvedAction::Web {
                url: "https://example.com/article",
            },
        ),
        (
            &["srz", "web", "https://youtu.be/5C_HPTJg5ek"][..],
            ResolvedAction::Web {
                url: "https://youtu.be/5C_HPTJg5ek",
            },
        ),
        (&["srz", "stdin"][..], ResolvedAction::Stdin),
        (
            &["srz", "document", "Cargo.toml"][..],
            ResolvedAction::Document {
                path: PathBuf::from("Cargo.toml"),
            },
        ),
        (
            &["srz", "document", "assets/document.pdf"][..],
            ResolvedAction::Document {
                path: PathBuf::from("assets/document.pdf"),
            },
        ),
        (
            &[
                "srz",
                "book",
                "The Great Gatsby",
                "--style",
                "concise",
                "--no-cache",
            ][..],
            ResolvedAction::Book {
                title: "The Great Gatsby",
                author: None,
            },
        ),
        (
            &["srz", "book", "9/11 Commission Report"][..],
            ResolvedAction::Book {
                title: "9/11 Commission Report",
                author: None,
            },
        ),
        (
            &["srz", "book", "Dune", "--author", "Frank Herbert"][..],
            ResolvedAction::Book {
                title: "Dune",
                author: Some("Frank Herbert"),
            },
        ),
        (
            &["srz", "book", "The Shipping News"][..],
            ResolvedAction::Book {
                title: "The Shipping News",
                author: None,
            },
        ),
        (
            &["srz", "news", "https://blog.rust-lang.org/feed.xml"][..],
            news_action("https://blog.rust-lang.org/feed.xml", NewsTimeframe::All, 5),
        ),
        (
            &["srz", "news", "current javascript news"][..],
            news_action("javascript", NewsTimeframe::All, 5),
        ),
        (
            &["srz", "news", "ai", "-t", "day", "-l", "8"][..],
            news_action("ai", NewsTimeframe::Day, 8),
        ),
        (
            &[
                "srz",
                "ask",
                "--web",
                "https://example.com",
                "What is the key takeaway?",
            ][..],
            ask_action(
                AskSource::Web("https://example.com".to_string()),
                Some("What is the key takeaway?"),
                false,
                false,
                None,
            ),
        ),
        (
            &["srz", "ask", "--document", "assets/document.pdf", "-i"][..],
            ask_action(
                AskSource::Document(PathBuf::from("assets/document.pdf")),
                None,
                true,
                false,
                None,
            ),
        ),
        (
            &[
                "srz",
                "ask",
                "--book",
                "The Hobbit",
                "--author",
                "J.R.R. Tolkien",
            ][..],
            ask_action(
                AskSource::Book {
                    title: "The Hobbit".to_string(),
                    author: Some("J.R.R. Tolkien".to_string()),
                },
                None,
                false,
                false,
                None,
            ),
        ),
        (
            &[
                "srz",
                "ask",
                "--library",
                "rust history",
                "When did 1.0 release?",
            ][..],
            ask_action(
                AskSource::LibraryQuery("rust history".to_string()),
                Some("When did 1.0 release?"),
                false,
                false,
                None,
            ),
        ),
        (
            &["srz", "ask", "What are the limitations?"][..],
            ask_action(
                AskSource::Latest,
                Some("What are the limitations?"),
                false,
                false,
                None,
            ),
        ),
        (
            &["srz", "ask", "--resume", "42"][..],
            ask_action(AskSource::Latest, None, true, false, Some(42)),
        ),
    ] {
        let cli = Cli::parse_from(argv);
        assert_eq!(cli.resolve_action().unwrap(), expected, "argv: {argv:?}");
    }

    // Subcommands pass through as ResolvedAction::Subcommand
    assert!(matches!(
        Cli::parse_from(["srz", "cache", "clean", "--reset-db"])
            .resolve_action()
            .unwrap(),
        ResolvedAction::Subcommand(Commands::Cache {
            action: CacheCommand::Clean { reset_db: true }
        })
    ));
    assert!(matches!(
        Cli::parse_from(["srz", "export", "-f", "json", "-o", "/tmp/out.json"])
            .resolve_action()
            .unwrap(),
        ResolvedAction::Subcommand(Commands::Export {
            format: ExportFormat::Json,
            ..
        })
    ));

    // Legacy implicit auto-detection and mutually exclusive ask sources fail parsing
    for bad_argv in [
        &["srz"][..],
        &["srz", "Dune"][..],
        &["srz", "https://example.com"][..],
        &["srz", "-"][..],
        &["srz", "Cargo.toml"][..],
        &["srz", "current javascript news"][..],
        &["srz", "doctur"][..],
        &["srz", "version"][..],
        &["srz", "--unknown-flag-xyz"][..],
        &["srz", "book", "Dune", "--style", "super-long"][..],
        &["srz", "completions", "unknown_shell"][..],
        &[
            "srz",
            "ask",
            "--web",
            "https://example.com",
            "--document",
            "Cargo.toml",
        ][..],
        &["srz", "ask", "target", "question"][..],
    ] {
        assert!(Cli::try_parse_from(bad_argv).is_err(), "{bad_argv:?}");
    }
}

#[test]
fn test_cli_global_flags_and_stream_mode() {
    let default_cli = Cli::parse_from(["srz", "book", "Dune"]);
    assert_eq!(default_cli.style, SummaryStyle::Concise);
    assert_eq!(default_cli.paging, PagingMode::Auto);
    assert_eq!(
        Cli::parse_from(["srz", "book", "Dune", "--style", "detailed"]).effective_max_tokens(),
        SummaryStyle::Detailed.default_max_tokens()
    );
    assert!(matches!(
        default_cli.stream_mode("Dune"),
        StreamMode::Buffered
    ));

    let stream_cli = Cli::parse_from(["srz", "book", "Dune", "--stream"]);
    assert!(matches!(
        stream_cli.stream_mode("Dune"),
        StreamMode::Stdout {
            header_title: "Dune"
        }
    ));
    for buffered in [
        &["srz", "book", "Dune", "--stream", "-o", "out.md"][..],
        &["srz", "book", "Dune", "--stream", "--json"][..],
    ] {
        assert!(matches!(
            Cli::parse_from(buffered).stream_mode("Dune"),
            StreamMode::Buffered
        ));
    }
}

#[test]
fn test_config_toml_deserialization_and_cli_merge() {
    let parse_with_config = |argv: &[&str], config: &Config| {
        let matches = Cli::command().get_matches_from(argv);
        let mut args = Cli::from_arg_matches(&matches).unwrap();
        args.apply_config(config, Some(&matches)).unwrap();
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
news_ttl_hours = 4
search_ttl_days = 10
summaries_ttl_days = 120
views_ttl_days = 5
auto_prune = false

[profiles.deep]
model = "deepseek-r1:14b"
style = "detailed"
context_window = 65536
"#;

    let config: Config = toml::from_str(toml_str).unwrap();
    let policy = config.cache.as_ref().unwrap().to_policy();
    assert_eq!(policy.max_bytes, 750 * 1024 * 1024);
    assert_eq!(policy.pages_ttl, Duration::from_secs(3 * 86400));
    assert_eq!(policy.news_ttl, Duration::from_secs(4 * 3600));
    assert_eq!(policy.auto_prune_interval, Duration::MAX);

    let synthetic = Config {
        settings: ProfileConfig {
            base_url: Some("http://ollama:11434/v1".into()),
            model: Some("custom-model:latest".into()),
            style: Some(SummaryStyle::Detailed),
            max_tokens: Some(2048),
            stream: Some(true),
            ..Default::default()
        },
        ..Default::default()
    };
    let applied = parse_with_config(&["srz", "book", "The Hobbit"], &synthetic);
    assert_eq!(applied.base_url, "http://ollama:11434/v1");
    assert_eq!(applied.effective_max_tokens(), 2048);
    assert!(applied.stream);

    // CLI > profile > root config precedence
    let cli_override = parse_with_config(
        &[
            "srz",
            "book",
            "The Hobbit",
            "--base-url",
            "http://cli/v1",
            "--profile",
            "deep",
        ],
        &config,
    );
    assert_eq!(cli_override.base_url, "http://cli/v1");
    assert_eq!(cli_override.model, "deepseek-r1:14b");
    assert_eq!(cli_override.style, SummaryStyle::Detailed);
    assert_eq!(cli_override.context_window, Some(65536));

    let bad_matches = Cli::command().get_matches_from(["srz", "book", "Dune", "--profile", "nope"]);
    let mut bad_args = Cli::from_arg_matches(&bad_matches).unwrap();
    assert!(bad_args.apply_config(&config, Some(&bad_matches)).is_err());
}

#[test]
fn test_cross_module_clients_and_assets_pdf() {
    let scraper = srz::sources::scraper::WebScraper::new().unwrap();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/document.pdf");
    let page = srz::sources::document::PdfExtractor::extract_file(&path).unwrap();
    assert!(page.content.contains("THE HISTORY OF RUST") && page.word_count >= 100);

    let doc = srz::sources::DocumentLoader::load_file(&path, &scraper).unwrap();
    assert!(doc.word_count >= 100 && doc.content.contains("CHRONOLOGICAL MILESTONE TIMELINE"));
}
