//! Checking what the user typed before it becomes a note.
//!
//! The capture overlay has a title field and a body field, and both are required (#213). The
//! first design (#4) used one field and made its first line the title, but a capture typed as
//! one line, which is how most are written, became all title and no body.
//!
//! The overlay checks the same rule before it offers the category picker. This check is the
//! one that holds: the command is a trust boundary, and nothing else stops an empty field.

/// What the user typed, resolved into the two things a note needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capture {
    pub title: String,
    pub body: String,
}

/// Why a capture cannot be filed. Both cases are the user's to fix, not errors to log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureError {
    /// The title is empty or whitespace. It names the note and its file.
    NoTitle,
    /// The body is empty or whitespace. A title alone is not the thought the user came to save.
    NoBody,
}

impl CaptureError {
    pub fn message(&self) -> &'static str {
        match self {
            Self::NoTitle => "Add a title.",
            Self::NoBody => "Add a body.",
        }
    }
}

/// Check both fields and tidy them for saving.
///
/// The title is trimmed on both ends: it becomes a file name, and stray spaces there are never
/// meant. The body loses only trailing whitespace. Leading blank lines and indentation inside it
/// are the user's formatting.
pub fn validate(title: &str, body: &str) -> Result<Capture, CaptureError> {
    let title = title.trim();
    if title.is_empty() {
        return Err(CaptureError::NoTitle);
    }
    let body = body.trim_end();
    if body.trim().is_empty() {
        return Err(CaptureError::NoBody);
    }
    Ok(Capture {
        title: title.to_string(),
        body: body.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::para::ParaCategory;

    #[test]
    fn the_title_and_body_are_kept_as_typed() {
        let capture =
            validate("Ring the dentist", "Before Friday, they close at noon").expect("valid");
        assert_eq!(capture.title, "Ring the dentist");
        assert_eq!(capture.body, "Before Friday, they close at noon");
    }

    #[test]
    fn the_title_is_trimmed_and_the_body_loses_only_trailing_whitespace() {
        let capture =
            validate("  Ring the dentist \t", "\n  indented\n\nSecond para \n\n").expect("valid");
        assert_eq!(capture.title, "Ring the dentist");
        assert_eq!(capture.body, "\n  indented\n\nSecond para");
    }

    #[test]
    fn an_empty_or_whitespace_title_is_refused() {
        for empty in ["", "   ", "\t \n"] {
            assert_eq!(
                validate(empty, "body"),
                Err(CaptureError::NoTitle),
                "{empty:?}"
            );
        }
    }

    #[test]
    fn an_empty_or_whitespace_body_is_refused() {
        for empty in ["", "   ", "\n\n", "  \n \t \n"] {
            assert_eq!(
                validate("Title", empty),
                Err(CaptureError::NoBody),
                "{empty:?}"
            );
        }
    }

    #[test]
    fn the_title_is_checked_first_when_both_are_empty() {
        // The overlay focuses the field named in the message, and the title comes first.
        assert_eq!(validate(" ", " "), Err(CaptureError::NoTitle));
    }

    #[test]
    fn the_overlay_chooses_a_category_through_the_vault_s_own_parser() {
        // ParaCategory::from_name already does this, case-insensitively, and is what the
        // rest of the vault files by. A second parser here would be one more place for
        // "Archive" to mean something different from "Archives".
        for category in ParaCategory::ALL {
            assert_eq!(
                ParaCategory::from_name(category.folder_name()),
                Some(category)
            );
        }
        assert_eq!(
            ParaCategory::from_name("projects"),
            Some(ParaCategory::Projects)
        );
        assert_eq!(ParaCategory::from_name("Inbox"), None);
    }
}
