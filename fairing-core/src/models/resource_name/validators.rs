use std::collections::HashSet;
use unicode_script::{Script, UnicodeScript};

pub trait ResourceIDValidator {
    fn validate(resource_id: &'_ str) -> Option<&'_ str>;
}

pub enum AnyValidator {}

impl ResourceIDValidator for AnyValidator {
    fn validate(resource_id: &'_ str) -> Option<&'_ str> {
        let resource_id = resource_id.split('/').next()?;

        if resource_id.len() > 1 && resource_id.len() < 128 {
            Some(resource_id)
        } else {
            None
        }
    }
}

/// Validator identifiers with Unicode support. Allows letters, numbers, `-` (dash), `.` (period)
/// and `_` (underscore).
///
/// Prevents mixing scripts using a (quite poor) implementation of moderately restrictive
/// rules here: https://www.unicode.org/reports/tr39/#moderately_restrictive
///
/// The normalize function should be run on input before inserting it into the database, or when
/// using user input.
///
/// This validator should be used for names where user expressiveness is important, but where there
/// cannot be any confusion between different identifiers.
pub enum UnicodeIdentifierValidator {}

impl UnicodeIdentifierValidator {
    /// Normalizes the resource ID using NFKC and case folding.
    ///
    /// The NFKC normalization does compatibility decomposition, ex. converts ℌ to H, or ① to 1.
    pub fn normalize(resource_id: &str) -> String {
        use caseless::Caseless;
        use unicode_normalization::UnicodeNormalization;

        resource_id.nfkc().default_case_fold().collect::<String>()
    }

    fn allowed_scripts() -> Vec<HashSet<Script>> {
        const ALLOWED_SCRIPTS: &[Script] = &[
            Script::Arabic,
            Script::Armenian,
            Script::Bengali,
            Script::Bopomofo,
            Script::Devanagari,
            Script::Ethiopic,
            Script::Georgian,
            Script::Gujarati,
            Script::Gurmukhi,
            Script::Han,
            Script::Hangul,
            Script::Hebrew,
            Script::Hiragana,
            Script::Kannada,
            Script::Katakana,
            Script::Khmer,
            Script::Lao,
            Script::Malayalam,
            Script::Myanmar,
            Script::Oriya,
            Script::Sinhala,
            Script::Tamil,
            Script::Telugu,
            Script::Thaana,
            Script::Thai,
            Script::Tibetan,
        ];

        fn hash_set(s: &[Script]) -> HashSet<Script> {
            s.iter().cloned().collect()
        }

        // Standard case, allow mixing any script from above with Latin, Common and Inherited.
        let mut allowed_scripts = ALLOWED_SCRIPTS
            .into_iter()
            .map(|&script| hash_set(&[script, Script::Latin, Script::Common, Script::Inherited]))
            .collect::<Vec<HashSet<_>>>();

        // Special cases for CJK scripts.
        allowed_scripts.push(hash_set(&[
            Script::Latin,
            Script::Han,
            Script::Hiragana,
            Script::Katakana,
            Script::Common,
            Script::Inherited,
        ]));

        allowed_scripts.push(hash_set(&[
            Script::Latin,
            Script::Han,
            Script::Bopomofo,
            Script::Common,
            Script::Inherited,
        ]));

        allowed_scripts.push(hash_set(&[
            Script::Latin,
            Script::Han,
            Script::Hangul,
            Script::Common,
            Script::Inherited,
        ]));

        // Cyrillic and Greek should not be mixed with Latin.
        allowed_scripts.push(hash_set(&[
            Script::Cyrillic,
            Script::Common,
            Script::Inherited,
        ]));

        allowed_scripts.push(hash_set(&[
            Script::Greek,
            Script::Common,
            Script::Inherited,
        ]));

        allowed_scripts
    }

    fn unallowed_scripts() -> Vec<HashSet<Script>> {
        let mut unallowed_scripts = Vec::new();

        unallowed_scripts.push(
            [Script::Common, Script::Inherited]
                .iter()
                .cloned()
                .collect(),
        );

        unallowed_scripts
    }
}

impl ResourceIDValidator for UnicodeIdentifierValidator {
    fn validate(resource_id: &'_ str) -> Option<&'_ str> {
        lazy_static::lazy_static! {
            // Allow unicode letters, numbers and a handful of separators.
            // https://unicode.org/reports/tr18/#General_Category_Property
            static ref RE: regex::Regex = regex::Regex::new(
                r"^[\p{letter}\p{number}]+([_.-][\p{letter}\p{number}]+)*$"
            ).unwrap();

            static ref ALLOWED_SCRIPTS: Vec<HashSet<Script>> = UnicodeIdentifierValidator::allowed_scripts();
            static ref UNALLOWED_SCRIPTS: Vec<HashSet<Script>> = UnicodeIdentifierValidator::unallowed_scripts();
        }

        let resource_id = resource_id.split('/').next()?;

        if resource_id.len() > 64 || !RE.is_match(resource_id) {
            return None;
        }

        // Block script mixing.
        let scripts = resource_id
            .chars()
            .map(|c| c.script())
            .collect::<HashSet<_>>();

        let is_allowed = ALLOWED_SCRIPTS
            .iter()
            .any(|allowed| scripts.is_subset(allowed));

        let is_unallowed = UNALLOWED_SCRIPTS
            .iter()
            .any(|unallowed| scripts.is_subset(unallowed));

        if is_allowed && !is_unallowed {
            Some(resource_id)
        } else {
            None
        }
    }
}

pub enum DomainLabelValidator {}

impl ResourceIDValidator for DomainLabelValidator {
    fn validate(resource_id: &'_ str) -> Option<&'_ str> {
        lazy_static::lazy_static! {
            static ref RE: regex::Regex = regex::Regex::new(
                r"^[a-z0-9]+(-[a-z0-9]+)*$"
            ).unwrap();
        }

        let resource_id = resource_id.split('/').next()?;

        if resource_id.len() < 64 && RE.is_match(resource_id) {
            Some(resource_id)
        } else {
            None
        }
    }
}

pub enum RevisionValidator {}

impl ResourceIDValidator for RevisionValidator {
    fn validate(resource_id: &'_ str) -> Option<&'_ str> {
        lazy_static::lazy_static! {
            static ref RE: regex::Regex = regex::Regex::new(
                r"^[^\n\t\\/]*$"
            ).unwrap();
        }

        let resource_id = resource_id.split('/').next()?;

        if resource_id.len() <= 200 && RE.is_match(resource_id) {
            Some(resource_id)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_identifier_normalize() {
        assert_eq!(UnicodeIdentifierValidator::normalize("Latin"), "latin");

        assert_eq!(UnicodeIdentifierValidator::normalize("한굴"), "한굴");

        assert_eq!(UnicodeIdentifierValidator::normalize("ℌ①"), "h1");
    }

    #[test]
    fn unicode_identifier_validate() {
        // Letters from a single script.
        assert!(UnicodeIdentifierValidator::validate("latin").is_some());
        assert!(UnicodeIdentifierValidator::validate("한굴").is_some());
        assert!(UnicodeIdentifierValidator::validate("Кириллица").is_some());

        // Mixed scripts.
        assert!(UnicodeIdentifierValidator::validate("latin.한굴").is_some());
        assert!(UnicodeIdentifierValidator::validate("latin.Кириллица").is_none());
        assert!(UnicodeIdentifierValidator::validate("latin.Ελληνικό").is_none());
        assert!(UnicodeIdentifierValidator::validate("한굴.Кириллица").is_none());

        // Unallowed characters.
        assert!(UnicodeIdentifierValidator::validate("⌘").is_none());
        assert!(UnicodeIdentifierValidator::validate(" ").is_none());

        // Numbers.
        assert!(UnicodeIdentifierValidator::validate("hello-123").is_some());
        assert!(UnicodeIdentifierValidator::validate("๑๒๓").is_some());
        assert!(UnicodeIdentifierValidator::validate("123").is_none());

        // Cannot start or end with separator characters.
        assert!(UnicodeIdentifierValidator::validate(".hello").is_none());
        assert!(UnicodeIdentifierValidator::validate("hello.").is_none());
    }

    #[test]
    fn domain_label_validate() {
        assert!(DomainLabelValidator::validate("test").is_some());
        assert!(DomainLabelValidator::validate("test-abc").is_some());
        assert!(DomainLabelValidator::validate("test-abc-def").is_some());

        assert!(DomainLabelValidator::validate("123").is_some());
        assert!(DomainLabelValidator::validate("a123").is_some());

        assert!(DomainLabelValidator::validate("-test").is_none());
        assert!(DomainLabelValidator::validate("test-").is_none());

        assert!(DomainLabelValidator::validate("-test-abc").is_none());
        assert!(DomainLabelValidator::validate("test-abc-").is_none());
    }
}
