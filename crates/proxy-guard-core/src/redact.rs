use std::path::Path;

pub fn redact_text(input: &str) -> String {
    let input = redact_user_profile(input);
    input
        .lines()
        .map(redact_line)
        .collect::<Vec<_>>()
        .join("\n")
}

fn redact_line(line: &str) -> String {
    let lower = line.to_ascii_lowercase();
    let header = [
        "proxy-authorization:",
        "authorization:",
        "set-cookie:",
        "cookie:",
    ]
    .iter()
    .filter_map(|needle| lower.find(needle).map(|index| (index, needle.len())))
    .min_by_key(|(index, _)| *index);
    if let Some((index, length)) = header {
        return format!("{} [REDACTED]", &line[..index + length]);
    }
    line.split_whitespace()
        .map(redact_token)
        .collect::<Vec<_>>()
        .join(" ")
}

fn redact_token(token: &str) -> String {
    let mut token = token.to_owned();
    if let Some(scheme_end) = token.find("://") {
        let authority_start = scheme_end + 3;
        let tail = &token[authority_start..];
        let authority_end = tail.find('/').unwrap_or(tail.len());
        let authority = &tail[..authority_end];
        if let Some(at) = authority.rfind('@') {
            let mut redacted = String::with_capacity(token.len());
            redacted.push_str(&token[..authority_start]);
            redacted.push_str("[REDACTED]@");
            redacted.push_str(&authority[at + 1..]);
            redacted.push_str(&tail[authority_end..]);
            token = redacted;
        }
    }

    let lower = token.to_ascii_lowercase();
    if let Some((index, needle)) = [
        "token=",
        "api_key=",
        "api-key=",
        "apikey=",
        "authorization=",
        "cookie=",
        "password=",
        "secret=",
    ]
    .iter()
    .filter_map(|needle| lower.find(needle).map(|index| (index, *needle)))
    .min_by_key(|(index, _)| *index)
    {
        let value_start = index + needle.len();
        return format!("{}[REDACTED]", &token[..value_start]);
    }
    token
}

pub fn display_path(path: &Path) -> String {
    redact_user_profile(&path.display().to_string())
}

fn redact_user_profile(input: &str) -> String {
    if let Ok(profile) = std::env::var("USERPROFILE")
        && !profile.is_empty()
    {
        return replace_case_insensitive(input, &profile, "%USERPROFILE%");
    }
    input.to_owned()
}

fn replace_case_insensitive(haystack: &str, needle: &str, replacement: &str) -> String {
    let lower_needle = needle.to_ascii_lowercase();
    let mut remaining = haystack;
    let mut rendered = String::with_capacity(haystack.len());
    loop {
        let lower_remaining = remaining.to_ascii_lowercase();
        let Some(index) = lower_remaining.find(&lower_needle) else {
            rendered.push_str(remaining);
            return rendered;
        };
        rendered.push_str(&remaining[..index]);
        rendered.push_str(replacement);
        remaining = &remaining[index + needle.len()..];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_proxy_credentials_and_secrets() {
        assert_eq!(
            redact_text("proxy=http://alice:secret@127.0.0.1:10808/path"),
            "proxy=http://[REDACTED]@127.0.0.1:10808/path"
        );
        assert_eq!(redact_text("token=abc123"), "token=[REDACTED]");
        assert_eq!(
            redact_text("Authorization: Bearer abc123"),
            "Authorization: [REDACTED]"
        );
        assert_eq!(redact_text("Cookie: session=abc123"), "Cookie: [REDACTED]");
        assert_eq!(
            redact_text("http://u:p@localhost/?token=abc123"),
            "http://[REDACTED]@localhost/?token=[REDACTED]"
        );
    }

    #[test]
    fn preserves_public_installation_guidance() {
        let message =
            "Install from https://chatgpt.com/download/ (Microsoft Store ID 9PLM9XGG6VKS)";
        assert_eq!(redact_text(message), message);
    }
}
