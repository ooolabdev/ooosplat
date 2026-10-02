use std::{net::IpAddr, path::PathBuf};

pub fn private_values(paths: &[PathBuf]) -> Vec<String> {
    let mut values = Vec::new();
    for path in paths {
        let text = path.to_string_lossy().into_owned();
        values.push(text.replace('\\', "/"));
        values.push(text);
        if let Some(name) = path.file_name() {
            values.push(name.to_string_lossy().into_owned());
        }
    }
    for key in ["USERNAME", "USER", "COMPUTERNAME", "HOSTNAME"] {
        if let Ok(value) = std::env::var(key) {
            values.push(value);
        }
    }
    values.retain(|value| !value.is_empty());
    values.sort_by_key(|value| std::cmp::Reverse(value.len()));
    values.dedup();
    values
}

fn suspicious(value: &str) -> bool {
    let value = value.trim_matches(|c: char| "\"'()[]{}<>,;".contains(c));
    let drive = value
        .as_bytes()
        .windows(3)
        .any(|v| v[0].is_ascii_alphabetic() && v[1] == b':' && (v[2] == b'\\' || v[2] == b'/'));
    if drive
        || value.starts_with('/')
        || value.contains("\\\\")
        || value.contains("://")
        || value.starts_with("www.")
        || value.contains('@')
        || value.contains('\\')
        || (value.contains('/')
            && value.chars().any(char::is_alphabetic)
            && !["WebGPU/WebGL2", "WebGL2/WebGPU"].contains(&value))
    {
        return true;
    }
    let network_value = value.rsplit('=').next().unwrap_or(value);
    let labelled = network_value
        .split_once(':')
        .map(|(_, value)| value)
        .unwrap_or(network_value);
    if [network_value, labelled].iter().any(|value| {
        value.parse::<IpAddr>().is_ok()
            || value
                .split(']')
                .next()
                .is_some_and(|v| v.trim_start_matches('[').parse::<IpAddr>().is_ok())
            || value
                .split(':')
                .next()
                .is_some_and(|v| v.parse::<IpAddr>().is_ok())
    }) {
        return true;
    }
    // A filename or DNS name may be relative and have no path prefix.
    value.rsplit_once('.').is_some_and(|(_, suffix)| {
        let suffix = suffix.trim_end_matches(|c: char| !c.is_ascii_alphanumeric());
        (2..=16).contains(&suffix.len())
            && suffix.chars().all(|c| c.is_ascii_alphabetic())
            && !["exe", "dll", "so", "dylib"].contains(&suffix.to_ascii_lowercase().as_str())
    })
}

pub fn sanitize(text: &str, private: &[String]) -> String {
    let mut cleaned = text.replace(|c: char| c.is_control() && c != '\n' && c != '\t', "");
    for value in private {
        // ASCII case folding keeps byte offsets stable even with non-ASCII paths.
        let lower = cleaned.to_ascii_lowercase();
        let needle = value.to_ascii_lowercase();
        for (offset, _) in lower
            .match_indices(&needle)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
        {
            // Short user/host names still need redaction, without matching letters inside technical words.
            if value.chars().count() < 3 {
                let before = cleaned[..offset].chars().next_back();
                let after = cleaned[offset + value.len()..].chars().next();
                if before.is_some_and(|c| c.is_alphanumeric() || c == '_')
                    || after.is_some_and(|c| c.is_alphanumeric() || c == '_')
                {
                    continue;
                }
            }
            cleaned.replace_range(offset..offset + value.len(), "[REDACTED]");
        }
    }
    cleaned
        .lines()
        .map(|line| {
            let lower = line
                .to_ascii_lowercase()
                .replace(char::is_whitespace, "")
                .replace(['\'', '"'], "");
            if [
                "authorization",
                "bearer",
                "password",
                "passwd",
                "secret",
                "api_key",
                "apikey",
                "access_token",
                "refresh_token",
                "token=",
                "token:",
                "username=",
                "hostname=",
                "username:",
                "hostname:",
                "user=",
                "user:",
                "serial",
                "uuid",
                "luid",
                "install_id",
                "installid",
            ]
            .iter()
            .any(|key| lower.contains(key))
            {
                return "[REDACTED_PRIVATE_LINE]".to_owned();
            }
            let mut result = String::new();
            let mut token = String::new();
            let mut quote = None;
            for c in line.chars().chain(std::iter::once(' ')) {
                if (c == '\'' || c == '"') && quote.is_none() {
                    quote = Some(c);
                    token.push(c);
                } else if quote == Some(c) {
                    quote = None;
                    token.push(c);
                } else if c.is_whitespace() && quote.is_none() {
                    if !token.is_empty() {
                        result.push_str(if suspicious(&token) {
                            "[REDACTED]"
                        } else {
                            &token
                        });
                        token.clear();
                    }
                    result.push(c);
                } else {
                    token.push(c);
                }
            }
            // Unterminated quoted strings must not evade path filtering.
            if !token.is_empty() {
                result.push_str(if suspicious(&token) {
                    "[REDACTED]"
                } else {
                    &token
                });
            }
            result.trim_end().to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn bounded(text: &str, bytes: usize) -> String {
    let mut start = text.len().saturating_sub(bytes);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    text[start..].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn removes_paths_files_hosts_network_addresses_and_credentials() {
        let text = "early eof at \"C:\\Users\\Alice\\private project\\frame.png\"\n/Users/alice/images/test.jpg failed\ninput=\\\\server\\share\\photo.png\ncontact alice@example.com 192.168.1.2 https://private.example/path\nAuthorization: Bearer SECRET\npassword=SECRET\nhostname=my-computer\nmy-file.png error Device Lost";
        let result = sanitize(text, &["Alice".into(), "my-computer".into()]);
        for secret in [
            "Alice",
            "alice",
            "private project",
            "test.jpg",
            "server",
            "192.168",
            "example",
            "SECRET",
            "my-computer",
            "my-file.png",
        ] {
            assert!(!result.contains(secret), "{result}");
        }
        assert!(result.contains("early eof"));
        assert!(result.contains("Device Lost"));
    }
    #[test]
    fn known_unicode_names_and_quoted_spaces_are_removed() {
        let text = "工程甲 failed at \"/tmp/目录 with space/photo.png\" and D:/user/data/file.jpg";
        let result = sanitize(text, &["工程甲".into()]);
        assert!(!result.contains("工程甲"));
        assert!(!result.contains("photo"));
        assert!(!result.contains("D:/"));
        assert!(bounded("中文abc", 4).len() <= 4);
    }

    #[test]
    fn removes_spaced_credentials_and_ipv6_addresses() {
        let result = sanitize("TOKEN = private-secret\nUser : private-user\nconnection [2001:db8::1]:443 failed\n10.20.30.40:8080 Device Lost", &[]);
        assert!(!result.contains("private-secret"));
        assert!(!result.contains("private-user"));
        assert!(!result.contains("2001:db8"));
        assert!(!result.contains("10.20.30.40"));
        assert!(result.contains("Device Lost"));
    }

    #[test]
    fn short_usernames_and_relative_directories_do_not_leak() {
        let result = sanitize(
            "by Al: early eof in work/frames/image.png\nWebGPU/WebGL2 allocation failed",
            &["Al".into()],
        );
        assert!(!result.contains("Al:"));
        assert!(!result.contains("work/frames"));
        assert!(result.contains("early eof"));
        assert!(result.contains("WebGPU/WebGL2 allocation"));
    }

    #[test]
    fn removes_json_credentials_device_identifiers_and_labelled_addresses() {
        let result = sanitize("{\"token\": \"private-secret\"}\n{\"username\": \"private-name\"}\nGPU UUID: device-identifier\naddress=192.168.1.10:443 failed\naddress:[2001:db8::1]:443 failed", &[]);
        for secret in [
            "private-secret",
            "private-name",
            "device-identifier",
            "192.168",
            "2001:db8",
        ] {
            assert!(!result.contains(secret), "{result}");
        }
    }
}
