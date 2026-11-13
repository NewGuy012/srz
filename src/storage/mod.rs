pub mod cache;
pub mod db;

pub use cache::{
    Cache, CachePolicy, CacheStats, PruneOptions, PruneStats, parse_bytes, parse_duration,
};
pub use db::{Database, tokenize_search_terms, tokenize_terms};

#[cfg(test)]
mod tests {
    use super::Database;
    use super::db::format_csv_cell;

    #[test]
    fn test_format_csv_cell_blocks_formula_bypasses() {
        // Leading whitespace / control characters before formula triggers
        for dangerous in [
            "\t=cmd|'/C calc'!A0",
            "\r=1+1",
            "\t123",
            "  =SUM(A1:A2)",
            " +cmd",
            "+inf",
            "-inf",
            "+NaN",
            "-nan",
            "--42",
            "+-42",
        ] {
            let formatted = format_csv_cell(dangerous);
            let unquoted = formatted.strip_prefix('"').unwrap_or(&formatted);
            assert!(
                unquoted.starts_with('\''),
                "expected leading apostrophe guard for {dangerous:?}, got {formatted:?}"
            );
        }

        // Valid finite signed numbers and normal text remain unprefixed
        assert_eq!(format_csv_cell("-42.5"), "-42.5");
        assert_eq!(format_csv_cell("+100"), "+100");
        assert_eq!(format_csv_cell("normal text"), "normal text");
        assert_eq!(format_csv_cell("hello, world"), "\"hello, world\"");
    }

    #[test]
    fn test_chat_session_id_bounds_validation() {
        let db = Database::open_in_memory().unwrap();

        for bad_id in [
            0_i64,
            -1,
            -5,
            i64::MIN,
            u32::MAX as i64,
            (u32::MAX as i64) + 1,
            i64::MAX,
        ] {
            assert!(
                db.add_chat_turn(bad_id, "q", "a").is_err(),
                "add_chat_turn should reject out-of-range id {bad_id}"
            );
            assert!(
                db.get_chat_session(bad_id).is_err(),
                "get_chat_session should reject out-of-range id {bad_id}"
            );
            assert!(
                db.delete_chat_session(bad_id).is_err(),
                "delete_chat_session should reject out-of-range id {bad_id}"
            );
        }

        // Valid boundaries within 1..u32::MAX succeed without error
        assert!(db.get_chat_session(1).unwrap().is_none());
        assert!(!db.delete_chat_session(1).unwrap());
        assert!(
            db.get_chat_session((u32::MAX as i64) - 1)
                .unwrap()
                .is_none()
        );
    }
}
