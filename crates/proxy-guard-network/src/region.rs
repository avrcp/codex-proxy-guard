use proxy_guard_core::CodexRegion;

/// Conservative name-based region pre-filter.
///
/// Import can only guess a node's region from its display name. Short codes are
/// matched as whole tokens (never as bare substrings), so `BUSINESS` is not
/// classified as `US`. The classifier returns at most one region, preferring the
/// first match in `JP > SG > US` order when a name is ambiguous.
#[derive(Clone, Copy, Debug, Default)]
pub struct RegionHintClassifier;

const JP_FLAGS: &[&str] = &["🇯🇵"];
const SG_FLAGS: &[&str] = &["🇸🇬"];
const US_FLAGS: &[&str] = &["🇺🇸"];

const JP_CJK: &[&str] = &["日本", "东京", "東京", "大阪"];
const SG_CJK: &[&str] = &["新加坡", "狮城", "獅城"];
const US_CJK: &[&str] = &["美国", "美國"];

const JP_TOKENS: &[&str] = &["JP", "JPN", "JAPAN", "TOKYO", "OSAKA"];
const SG_TOKENS: &[&str] = &["SG", "SGP", "SINGAPORE"];
const US_TOKENS: &[&str] = &["US", "USA", "SEATTLE", "DALLAS", "CHICAGO"];

const US_PHRASES: &[&[&str]] = &[
    &["UNITED", "STATES"],
    &["LOS", "ANGELES"],
    &["SAN", "JOSE"],
    &["NEW", "YORK"],
];

impl RegionHintClassifier {
    #[must_use]
    pub fn classify(name: &str) -> Option<CodexRegion> {
        if name.trim().is_empty() {
            return None;
        }
        if JP_FLAGS.iter().any(|flag| name.contains(flag))
            || JP_CJK.iter().any(|word| name.contains(word))
        {
            return Some(CodexRegion::JP);
        }
        if SG_FLAGS.iter().any(|flag| name.contains(flag))
            || SG_CJK.iter().any(|word| name.contains(word))
        {
            return Some(CodexRegion::SG);
        }
        if US_FLAGS.iter().any(|flag| name.contains(flag))
            || US_CJK.iter().any(|word| name.contains(word))
        {
            return Some(CodexRegion::US);
        }

        let tokens = tokenize(name);
        for phrase in US_PHRASES {
            if contains_phrase(&tokens, phrase) {
                return Some(CodexRegion::US);
            }
        }
        if tokens
            .iter()
            .any(|token| JP_TOKENS.contains(&token.as_str()))
        {
            return Some(CodexRegion::JP);
        }
        if tokens
            .iter()
            .any(|token| SG_TOKENS.contains(&token.as_str()))
        {
            return Some(CodexRegion::SG);
        }
        if tokens
            .iter()
            .any(|token| US_TOKENS.contains(&token.as_str()))
        {
            return Some(CodexRegion::US);
        }
        None
    }
}

fn tokenize(name: &str) -> Vec<String> {
    name.split(|character: char| !character.is_ascii_alphanumeric())
        .map(str::to_ascii_uppercase)
        .filter(|token| !token.is_empty())
        .collect()
}

fn contains_phrase(tokens: &[String], phrase: &[&str]) -> bool {
    if phrase.len() > tokens.len() {
        return false;
    }
    tokens
        .windows(phrase.len())
        .any(|window| window.iter().zip(phrase).all(|(token, word)| token == word))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_jp_aliases() {
        for name in [
            "JP",
            "JPN",
            "JAPAN",
            "日本",
            "東京",
            "东京",
            "Tokyo",
            "TOKYO",
            "大阪",
            "OSAKA",
            "🇯🇵 Tokyo",
        ] {
            assert_eq!(
                RegionHintClassifier::classify(name),
                Some(CodexRegion::JP),
                "{name} should be JP"
            );
        }
    }

    #[test]
    fn classifies_sg_aliases() {
        for name in [
            "SG",
            "SGP",
            "SINGAPORE",
            "新加坡",
            "狮城",
            "獅城",
            "🇸🇬 Singapore",
        ] {
            assert_eq!(
                RegionHintClassifier::classify(name),
                Some(CodexRegion::SG),
                "{name} should be SG"
            );
        }
    }

    #[test]
    fn classifies_us_aliases() {
        for name in [
            "US",
            "USA",
            "UNITED STATES",
            "美国",
            "美國",
            "LOS ANGELES",
            "SAN JOSE",
            "SEATTLE",
            "NEW YORK",
            "DALLAS",
            "CHICAGO",
            "🇺🇸 Seattle",
        ] {
            assert_eq!(
                RegionHintClassifier::classify(name),
                Some(CodexRegion::US),
                "{name} should be US"
            );
        }
    }

    #[test]
    fn ignores_non_jp_sg_us_names() {
        for name in [
            "BUSINESS", "Germany", "HK", "TW", "KR", "London", "UK", "AUSTIN", "CA",
        ] {
            assert_eq!(
                RegionHintClassifier::classify(name),
                None,
                "{name} should not match"
            );
        }
    }

    #[test]
    fn short_codes_match_token_boundaries_only() {
        assert_eq!(
            RegionHintClassifier::classify("JP-Tokyo-01"),
            Some(CodexRegion::JP)
        );
        assert_eq!(
            RegionHintClassifier::classify("SG 2"),
            Some(CodexRegion::SG)
        );
        assert_eq!(
            RegionHintClassifier::classify("US_West"),
            Some(CodexRegion::US)
        );
        // No separator means no token boundary: conservative None.
        assert_eq!(RegionHintClassifier::classify("JP01"), None);
        assert_eq!(RegionHintClassifier::classify("BUSINESS"), None);
    }
}
