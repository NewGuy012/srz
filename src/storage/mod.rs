pub mod cache;
pub mod db;

#[cfg(test)]
fn make_summary<'a>(
    target_type: &'a str,
    title: &'a str,
    author: Option<&'a str>,
    url: Option<&'a str>,
    summary: &'a str,
) -> db::NewSummaryRecord<'a> {
    db::NewSummaryRecord {
        target_type,
        title,
        author,
        url,
        style: "concise",
        model: "gemma-4",
        word_count: 500,
        prompt_tokens: None,
        completion_tokens: None,
        duration_ms: 100,
        tokens_per_second: None,
        summary,
        reasoning: None,
    }
}
