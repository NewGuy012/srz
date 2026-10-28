use clap::Parser;
use srz::cli::{CacheCommand, Cli, ResolvedAction, SummaryStyle};

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
}

#[test]
fn test_default_style_is_concise() {
    let cli = Cli::parse_from(["srz", "Dune"]);
    assert_eq!(cli.style, SummaryStyle::Concise);
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
