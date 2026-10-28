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

    let models_cli = Cli::parse_from(["srz", "models"]);
    assert_eq!(models_cli.resolve_action().unwrap(), ResolvedAction::Models);

    let doctor_cli = Cli::parse_from(["srz", "doctor"]);
    assert_eq!(doctor_cli.resolve_action().unwrap(), ResolvedAction::Doctor);
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
