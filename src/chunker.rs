pub struct TextChunker;

impl TextChunker {
    /// Splits text into chunks of roughly `max_words` with `overlap_words` overlap.
    /// Preserves paragraph boundaries wherever possible.
    pub fn chunk(text: &str, max_words: usize, overlap_words: usize) -> Vec<String> {
        let paragraphs: Vec<&str> = text
            .split("\n\n")
            .map(|p| p.trim())
            .filter(|p| !p.is_empty())
            .collect();

        if paragraphs.is_empty() {
            return vec![text.to_string()];
        }

        let total_words: usize = paragraphs.iter().map(|p| p.split_whitespace().count()).sum();
        if total_words <= max_words {
            return vec![text.to_string()];
        }

        let mut chunks = Vec::new();
        let mut current_chunk_paragraphs: Vec<&str> = Vec::new();
        let mut current_word_count = 0;

        for para in &paragraphs {
            let p_words = para.split_whitespace().count();

            if current_word_count + p_words > max_words && !current_chunk_paragraphs.is_empty() {
                chunks.push(current_chunk_paragraphs.join("\n\n"));

                // Calculate overlap paragraphs to retain
                let mut retained = Vec::new();
                let mut retained_words = 0;
                for prev_p in current_chunk_paragraphs.iter().rev() {
                    let w = prev_p.split_whitespace().count();
                    if retained_words + w <= overlap_words {
                        retained.push(*prev_p);
                        retained_words += w;
                    } else {
                        break;
                    }
                }
                retained.reverse();

                current_chunk_paragraphs = retained;
                current_word_count = retained_words;
            }

            current_chunk_paragraphs.push(*para);
            current_word_count += p_words;
        }

        if !current_chunk_paragraphs.is_empty() {
            chunks.push(current_chunk_paragraphs.join("\n\n"));
        }

        chunks
    }
}

