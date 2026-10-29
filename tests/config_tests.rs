use clap::{CommandFactory, FromArgMatches};
use srz::cli::{Cli, ConfigSubcommand, ResolvedAction, SummaryStyle};
use srz::config::{Config, ProfileConfig};

#[test]
fn test_config_subcommand_resolution() {
    let path_cli = Cli::command().get_matches_from(["srz", "config", "path"]);
    let path_args = Cli::from_arg_matches(&path_cli).unwrap();
    assert_eq!(
        path_args.resolve_action().unwrap(),
        ResolvedAction::Config(ConfigSubcommand::Path)
    );

    let show_cli = Cli::command().get_matches_from(["srz", "config", "show"]);
    let show_args = Cli::from_arg_matches(&show_cli).unwrap();
    assert_eq!(
        show_args.resolve_action().unwrap(),
        ResolvedAction::Config(ConfigSubcommand::Show)
    );

    let init_cli = Cli::command().get_matches_from(["srz", "config", "init"]);
    let init_args = Cli::from_arg_matches(&init_cli).unwrap();
    assert_eq!(
        init_args.resolve_action().unwrap(),
        ResolvedAction::Config(ConfigSubcommand::Init)
    );
}

#[test]
fn test_config_deserialization_and_profiles() {
    let toml_str = r#"
base_url = "http://localhost:11434/v1"
model = "llama3:8b"
style = "bullets"
max_tokens = 4096

[profiles.fast]
model = "qwen2.5:3b"
base_url = "http://localhost:9931/v1"
stream = true

[profiles.deep]
model = "deepseek-r1:14b"
style = "detailed"
"#;

    let config: Config = toml::from_str(toml_str).unwrap();
    assert_eq!(
        config.base_url.as_deref(),
        Some("http://localhost:11434/v1")
    );
    assert_eq!(config.model.as_deref(), Some("llama3:8b"));
    assert_eq!(config.style, Some(SummaryStyle::Bullets));
    assert_eq!(config.max_tokens, Some(4096));

    let fast = config.profiles.get("fast").unwrap();
    assert_eq!(fast.model.as_deref(), Some("qwen2.5:3b"));
    assert_eq!(fast.base_url.as_deref(), Some("http://localhost:9931/v1"));
    assert_eq!(fast.stream, Some(true));

    let deep = config.profiles.get("deep").unwrap();
    assert_eq!(deep.model.as_deref(), Some("deepseek-r1:14b"));
    assert_eq!(deep.style, Some(SummaryStyle::Detailed));
}

#[test]
fn test_config_applied_to_cli_when_not_specified_on_command_line() {
    let config = Config {
        settings: ProfileConfig {
            base_url: Some("http://ollama:11434/v1".to_string()),
            model: Some("custom-model:latest".to_string()),
            style: Some(SummaryStyle::Detailed),
            max_tokens: Some(2048),
            stream: Some(true),
            concurrency: Some(3),
            ..Default::default()
        },
        ..Default::default()
    };

    let matches = Cli::command().get_matches_from(["srz", "The Hobbit"]);
    let mut args = Cli::from_arg_matches(&matches).unwrap();

    args.apply_config(&config, Some(&matches));

    assert_eq!(args.base_url, "http://ollama:11434/v1");
    assert_eq!(args.model, "custom-model:latest");
    assert_eq!(args.style, SummaryStyle::Detailed);
    assert_eq!(args.max_tokens, 2048);
    assert!(args.stream);
    assert_eq!(args.concurrency, 3);
}

#[test]
fn test_cli_flags_take_precedence_over_config() {
    let config = Config {
        settings: ProfileConfig {
            base_url: Some("http://config-url/v1".to_string()),
            model: Some("config-model".to_string()),
            style: Some(SummaryStyle::Detailed),
            ..Default::default()
        },
        ..Default::default()
    };

    let matches = Cli::command().get_matches_from([
        "srz",
        "The Hobbit",
        "--base-url",
        "http://cli-url/v1",
        "--style",
        "bullets",
    ]);
    let mut args = Cli::from_arg_matches(&matches).unwrap();

    args.apply_config(&config, Some(&matches));

    // CLI overrides config:
    assert_eq!(args.base_url, "http://cli-url/v1");
    assert_eq!(args.style, SummaryStyle::Bullets);
    // Config still populates unsupplied flags:
    assert_eq!(args.model, "config-model");
}

#[test]
fn test_profile_flag_overrides_root_config() {
    let mut config = Config {
        settings: ProfileConfig {
            model: Some("root-model".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };

    let profile = ProfileConfig {
        model: Some("profile-model".to_string()),
        style: Some(SummaryStyle::Detailed),
        ..Default::default()
    };
    config.profiles.insert("deep".to_string(), profile);

    let matches = Cli::command().get_matches_from(["srz", "The Hobbit", "--profile", "deep"]);
    let mut args = Cli::from_arg_matches(&matches).unwrap();

    args.apply_config(&config, Some(&matches));

    assert_eq!(args.model, "profile-model");
    assert_eq!(args.style, SummaryStyle::Detailed);
}

#[test]
fn test_cache_config_deserialization_and_policy() {
    use std::time::Duration;

    let toml_str = r#"
base_url = "http://localhost:11434/v1"

[cache]
max_size_mb = 750
pages_ttl_days = 3
search_ttl_days = 10
summaries_ttl_days = 120
views_ttl_days = 5
auto_prune = false
"#;

    let config: Config = toml::from_str(toml_str).unwrap();
    let cache_cfg = config.cache.expect("Cache config should be present");
    assert_eq!(cache_cfg.max_size_mb, Some(750));
    assert_eq!(cache_cfg.pages_ttl_days, Some(3));
    assert_eq!(cache_cfg.search_ttl_days, Some(10));
    assert_eq!(cache_cfg.summaries_ttl_days, Some(120));
    assert_eq!(cache_cfg.views_ttl_days, Some(5));
    assert_eq!(cache_cfg.auto_prune, Some(false));

    let policy = cache_cfg.to_policy();
    assert_eq!(policy.max_bytes, 750 * 1024 * 1024);
    assert_eq!(policy.pages_ttl, Duration::from_secs(3 * 86400));
    assert_eq!(policy.search_ttl, Duration::from_secs(10 * 86400));
    assert_eq!(policy.summaries_ttl, Duration::from_secs(120 * 86400));
    assert_eq!(policy.views_ttl, Duration::from_secs(5 * 86400));
    assert_eq!(policy.auto_prune_interval, Duration::MAX);
}
