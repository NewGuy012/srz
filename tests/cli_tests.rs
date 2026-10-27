use clap::Parser;
use srz::cli::{Cli, ResolvedAction, SummaryStyle};

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
            assert_eq!(author.as_deref(), Some("F. Scott Fitzgerald"));
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
            assert_eq!(author.as_deref(), Some("Frank Herbert"));
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
