//! Label naming, matching how Loki ingests OTLP (design 6.2).

/// A Loki label name for an attribute key: every character outside
/// `[A-Za-z0-9_]` becomes `_` (`service.name` becomes `service_name`).
pub fn loki_label_name(key: &str) -> String {
    key.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}
