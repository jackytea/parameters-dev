pub const KEYWORDS: [&str; 30] = [
    "workflow",
    "define",
    "node",
    "step",
    "uses",
    "input",
    "output",
    "note",
    "is",
    "secret",
    "true",
    "false",
    "after",
    "when",
    "and",
    "or",
    "not",
    "succeeded",
    "failed",
    "skipped",
    "retry",
    "times",
    "timeout",
    "concurrency",
    "for",
    "each",
    "in",
    "import",
    "as",
    "from",
];

pub const MAX_NAME_LENGTH: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameKind {
    Step,
    Input,
    Output,
    Field,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameProblem {
    Empty,
    TooLong,
    InvalidCharacters,
    Keyword,
}

#[must_use]
pub fn is_keyword(word: &str) -> bool {
    KEYWORDS.contains(&word)
}

#[must_use]
pub fn is_identifier(word: &str) -> bool {
    let mut bytes = word.bytes();
    bytes
        .next()
        .is_some_and(|first| first.is_ascii_lowercase() || first == b'_')
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

pub fn check_name(word: &str) -> Result<(), NameProblem> {
    if word.is_empty() {
        Err(NameProblem::Empty)
    } else if word.len() > MAX_NAME_LENGTH {
        Err(NameProblem::TooLong)
    } else if !is_identifier(word) {
        Err(NameProblem::InvalidCharacters)
    } else if is_keyword(word) {
        Err(NameProblem::Keyword)
    } else {
        Ok(())
    }
}

#[must_use]
pub fn is_valid_name(word: &str) -> bool {
    check_name(word).is_ok()
}

#[must_use]
pub fn is_valid_node_type(name: &str) -> bool {
    name.len() <= MAX_NAME_LENGTH * 2
        && name.split('.').count() >= 2
        && name.split('.').all(is_identifier)
}

#[must_use]
pub fn suggest_name(word: &str, kind: NameKind) -> Option<String> {
    let mut candidate: String = word
        .chars()
        .map(|character| match character.to_ascii_lowercase() {
            lower @ ('a'..='z' | '0'..='9' | '_') => lower,
            _ => '_',
        })
        .collect();
    if candidate.starts_with(|first: char| first.is_ascii_digit()) {
        candidate.insert(0, '_');
    }
    candidate.truncate(MAX_NAME_LENGTH);
    if is_keyword(&candidate) {
        candidate.push_str(match kind {
            NameKind::Step => "_step",
            NameKind::Input | NameKind::Output | NameKind::Field => "_value",
        });
    }
    (is_valid_name(&candidate) && candidate != word).then_some(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_are_lowercase_snake_case() {
        assert!(is_identifier("build"));
        assert!(is_identifier("_private"));
        assert!(is_identifier("step_2"));
        assert!(!is_identifier(""));
        assert!(!is_identifier("2step"));
        assert!(!is_identifier("Build"));
        assert!(!is_identifier("build-site"));
    }

    #[test]
    fn keywords_are_not_valid_names() {
        assert_eq!(check_name("timeout"), Err(NameProblem::Keyword));
        assert_eq!(check_name("import"), Err(NameProblem::Keyword));
        assert_eq!(check_name("checkout"), Ok(()));
    }

    #[test]
    fn names_have_a_length_limit() {
        assert_eq!(check_name(&"a".repeat(MAX_NAME_LENGTH)), Ok(()));
        assert_eq!(
            check_name(&"a".repeat(MAX_NAME_LENGTH + 1)),
            Err(NameProblem::TooLong)
        );
    }

    #[test]
    fn node_types_are_qualified() {
        assert!(is_valid_node_type("http.request"));
        assert!(is_valid_node_type("workflow.call"));
        assert!(is_valid_node_type("acme.deploy.site"));
        assert!(!is_valid_node_type("request"));
        assert!(!is_valid_node_type("http."));
        assert!(!is_valid_node_type("Http.request"));
    }

    #[test]
    fn suggestions_fix_names() {
        assert_eq!(
            suggest_name("Build-Site", NameKind::Step),
            Some("build_site".into())
        );
        assert_eq!(
            suggest_name("timeout", NameKind::Field),
            Some("timeout_value".into())
        );
        assert_eq!(
            suggest_name("after", NameKind::Step),
            Some("after_step".into())
        );
        assert_eq!(suggest_name("2fa", NameKind::Input), Some("_2fa".into()));
        assert_eq!(suggest_name("fine", NameKind::Input), None);
    }
}
