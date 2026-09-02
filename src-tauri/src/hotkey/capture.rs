//! Turning what the user typed into a note's title and body.
//!
//! The capture overlay is one text field, not a title field and a body field. Asking for a
//! title is a second decision at the moment the point is to have none: the user pressed a
//! hotkey because a thought was in the way, and every field between them and putting it down
//! is friction the whole feature exists to remove.
//!
//! So the first line becomes the title and the rest becomes the body, which is the convention
//! the vault already uses everywhere else.

/// What the user typed, resolved into the two things a note needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capture {
    pub title: String,
    pub body: String,
}

/// Why a capture cannot be filed. Both cases are the user's to fix, not errors to log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureError {
    /// Nothing but whitespace. Saving it would file an empty note the user never sees again.
    Empty,
}

impl CaptureError {
    pub fn message(&self) -> &'static str {
        match self {
            Self::Empty => "There is nothing to save yet.",
        }
    }
}

/// Split what was typed into a title and a body.
///
/// Leading blank lines are skipped rather than becoming an empty title: a paste often carries
/// them, and an empty title would produce a note named after nothing.
pub fn split(text: &str) -> Result<Capture, CaptureError> {
    let mut lines = text.lines().skip_while(|line| line.trim().is_empty());
    let title = lines.next().map(str::trim).unwrap_or_default();
    if title.is_empty() {
        return Err(CaptureError::Empty);
    }

    // Only the newline that separated title from body is consumed. Blank lines *inside* the
    // body are the user's paragraphs and are left exactly as typed.
    let body = lines.collect::<Vec<_>>().join("\n");

    Ok(Capture {
        title: title.to_string(),
        body: body.trim_end().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::para::ParaCategory;

    #[test]
    fn the_first_line_becomes_the_title_and_the_rest_the_body() {
        let capture = split("Ring the dentist\nBefore Friday, they close at noon").expect("splits");
        assert_eq!(capture.title, "Ring the dentist");
        assert_eq!(capture.body, "Before Friday, they close at noon");
    }

    #[test]
    fn a_single_line_is_all_title_and_no_body() {
        let capture = split("Ring the dentist").expect("splits");
        assert_eq!(capture.title, "Ring the dentist");
        assert_eq!(capture.body, "");
    }

    #[test]
    fn leading_blank_lines_do_not_become_an_empty_title() {
        // Pasted text very often starts with them, and an empty title names the file after
        // nothing.
        let capture = split("\n\n  \nRing the dentist\nbody").expect("splits");
        assert_eq!(capture.title, "Ring the dentist");
        assert_eq!(capture.body, "body");
    }

    #[test]
    fn blank_lines_inside_the_body_are_the_users_paragraphs_and_survive() {
        let capture = split("Title\n\nFirst para\n\nSecond para").expect("splits");
        assert_eq!(capture.body, "\nFirst para\n\nSecond para");
    }

    #[test]
    fn whitespace_alone_is_refused_rather_than_filed() {
        for empty in ["", "   ", "\n\n", "  \n \t \n"] {
            assert_eq!(split(empty), Err(CaptureError::Empty), "{empty:?}");
        }
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
