const MAX_NOTIFICATION_LEN: usize = 180;

pub fn short_notification_text(message: &str) -> String {
    let message = message.trim();
    if message.is_empty() {
        return String::new();
    }

    let message = message.lines().next().unwrap_or(message).trim();
    let shortened = shorten_colon_chain(message);

    if char_len(&shortened) <= MAX_NOTIFICATION_LEN {
        shortened
    } else {
        truncate_chars(&shortened, MAX_NOTIFICATION_LEN - 1) + "…"
    }
}

/// "Install failed: a: b: timed out" -> "Install failed: timed out"
fn shorten_colon_chain(message: &str) -> String {
    let Some((prefix, rest)) = message.split_once(": ") else {
        return message.to_string();
    };

    if !rest.contains(": ") {
        return message.to_string();
    }

    let root = rest.rsplit(": ").next().unwrap_or(rest).trim();
    if root.is_empty() {
        message.to_string()
    } else {
        format!("{prefix}: {root}")
    }
}

fn char_len(text: &str) -> usize {
    text.chars().count()
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}
