//! Construction of the context handed to the decision model.
//!
//! The model receives a passive description of the upstream commit: its
//! metadata, the list of touched files and a truncated patch. The text is
//! inserted verbatim into the `<donnees>` block of the Foq prompt.

use fsrs_core::UpstreamCommit;

/// Maximum number of characters of the patch included in the context.
///
/// The runtime is started with a 4096 token context window; the patch is
/// therefore truncated so that the prompt always fits.
pub const MAX_PATCH_CHARS: usize = 6000;

/// Maximum number of file names listed in the context.
pub const MAX_LISTED_FILES: usize = 40;

/// Build the context describing one upstream commit.
pub fn build_context(commit: &UpstreamCommit, patch: Option<&str>) -> String {
    let mut context = String::new();
    context.push_str(&format!("Commit: {}\n", commit.sha));
    context.push_str(&format!("Auteur: {}", commit.author));
    if !commit.author_email.is_empty() {
        context.push_str(&format!(" <{}>", commit.author_email));
    }
    context.push('\n');
    context.push_str(&format!("Date: {}\n", commit.authored_at));
    context.push_str(&format!(
        "Fichiers modifiés: {} (+{} -{})\n",
        commit.files.len(),
        commit.insertions,
        commit.deletions
    ));
    for file in commit.files.iter().take(MAX_LISTED_FILES) {
        context.push_str(&format!("  - {file}\n"));
    }
    if commit.files.len() > MAX_LISTED_FILES {
        context.push_str(&format!(
            "  … et {} autres fichiers\n",
            commit.files.len() - MAX_LISTED_FILES
        ));
    }
    context.push_str(&format!("Titre: {}\n", commit.summary));
    if !commit.body.is_empty() {
        context.push_str(&format!("Description:\n{}\n", commit.body));
    }
    if let Some(patch) = patch {
        let trimmed = truncate(patch, MAX_PATCH_CHARS);
        if !trimmed.is_empty() {
            context.push_str(&format!("Diff:\n{trimmed}\n"));
        }
    }
    context
}

/// Truncate `text` to `limit` characters, marking the cut explicitly.
pub fn truncate(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let mut end = limit;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n… (diff tronqué)", &text[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit() -> UpstreamCommit {
        UpstreamCommit {
            repo_id: 1,
            sha: "a".repeat(40),
            short_sha: "aaaaaaaa".to_string(),
            summary: "Fix panic on empty input".to_string(),
            body: "The parser used to panic.".to_string(),
            author: "Ada".to_string(),
            author_email: "ada@example.com".to_string(),
            authored_at: 1_700_000_000,
            committed_at: 1_700_000_000,
            files: vec!["src/lib.rs".to_string()],
            insertions: 3,
            deletions: 1,
            url: "https://example.com/commit".to_string(),
        }
    }

    #[test]
    fn context_contains_the_essential_metadata() {
        let context = build_context(&commit(), None);
        assert!(context.contains("Fix panic on empty input"));
        assert!(context.contains("Ada <ada@example.com>"));
        assert!(context.contains("src/lib.rs"));
        assert!(context.contains("(+3 -1)"));
        assert!(context.contains("The parser used to panic."));
    }

    #[test]
    fn context_includes_the_patch_when_available() {
        let context = build_context(&commit(), Some("+let x = 1;\n"));
        assert!(context.contains("Diff:\n+let x = 1;"));
    }

    #[test]
    fn long_patches_are_truncated_on_a_character_boundary() {
        let patch = "é".repeat(MAX_PATCH_CHARS);
        let trimmed = truncate(&patch, 10);
        assert!(trimmed.ends_with("… (diff tronqué)"));
        // 10 bytes of a two-byte character can only hold 5 characters.
        assert!(trimmed.starts_with("ééééé"));
    }

    #[test]
    fn short_patches_are_kept_verbatim() {
        assert_eq!(truncate("abc", 10), "abc");
    }

    #[test]
    fn only_a_bounded_number_of_files_is_listed() {
        let mut many = commit();
        many.files = (0..100).map(|i| format!("file{i}.rs")).collect();
        let context = build_context(&many, None);
        assert!(context.contains("file0.rs"));
        assert!(!context.contains("file50.rs"));
        assert!(context.contains("et 60 autres fichiers"));
    }
}
