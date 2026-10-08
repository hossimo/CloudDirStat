use std::borrow::Cow;

use crate::Cost;

const UNITS: [&str; 7] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];

pub fn format_bytes(bytes: u64) -> String {
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }

    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// `count` with thousands separators and `noun`, plural unless there is exactly one:
/// `1 object`, `1,234 objects`.
pub fn format_counted(count: u64, noun: &str) -> String {
    let plural = if count == 1 { "" } else { "s" };
    format!("{} {noun}{plural}", format_count(count))
}

pub fn format_count(count: u64) -> String {
    let digits = count.to_string();
    let mut formatted = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            formatted.push(',');
        }
        formatted.push(digit);
    }
    formatted
}

/// Shown where costs appear, so they aren't mistaken for a bill or a local currency.
pub const PRICE_NOTE: &str = "Costs are estimates at public list prices in US dollars, \
     before tax, discounts, and free tiers.";

/// US dollars with cents, e.g. `US$1,234.56`. Tiny nonzero amounts show as `<US$0.01`.
pub fn format_usd(cost: Cost) -> String {
    let cents = (cost.usd() * 100.0).round() as u64;
    if cents == 0 && cost > Cost::ZERO {
        return "<US$0.01".to_owned();
    }
    format!("US${}.{:02}", format_count(cents / 100), cents % 100)
}

/// US dollars to four decimals, e.g. `US$0.0211`, for request costs that are often
/// fractions of a cent.
pub fn format_usd_fine(usd: f64) -> String {
    format!("US${usd:.4}")
}

/// `text` with control characters and bidirectional formatting characters escaped
/// (`\u{1b}`, `\n`, `\u{202e}`), so that object names, which anyone who can write to a
/// bucket chooses, can't send escape sequences to a terminal or reorder what it shows.
pub fn escape_control(text: &str) -> Cow<'_, str> {
    if !text.chars().any(needs_escape) {
        return Cow::Borrowed(text);
    }
    let mut escaped = String::with_capacity(text.len() + 8);
    for c in text.chars() {
        if needs_escape(c) {
            escaped.extend(c.escape_default());
        } else {
            escaped.push(c);
        }
    }
    Cow::Owned(escaped)
}

fn needs_escape(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_control_characters() {
        assert_eq!(escape_control("logs/app.log"), "logs/app.log");
        assert!(matches!(escape_control("ünïcödé 名前"), Cow::Borrowed(_)));
        assert_eq!(escape_control("a\x1b[31mred"), "a\\u{1b}[31mred");
        assert_eq!(
            escape_control("\x1b]8;;http://x\x07link"),
            "\\u{1b}]8;;http://x\\u{7}link"
        );
        assert_eq!(escape_control("two\nlines\r"), "two\\nlines\\r");
        assert_eq!(escape_control("del\x7f c1\u{9b}"), "del\\u{7f} c1\\u{9b}");
        assert_eq!(
            escape_control("evil\u{202e}txt.exe"),
            "evil\\u{202e}txt.exe"
        );
    }

    #[test]
    fn counts_nouns() {
        assert_eq!(format_counted(0, "object"), "0 objects");
        assert_eq!(format_counted(1, "object"), "1 object");
        assert_eq!(format_counted(1234, "warning"), "1,234 warnings");
    }

    #[test]
    fn formats_bytes_with_binary_units() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(1024), "1.0 KiB");
        assert_eq!(format_bytes(1536), "1.5 KiB");
        assert_eq!(format_bytes(5 * 1024 * 1024 * 1024), "5.0 GiB");
    }

    #[test]
    fn formats_dollars_with_cents() {
        assert_eq!(format_usd(Cost::ZERO), "US$0.00");
        assert_eq!(format_usd(Cost::from_usd(0.001)), "<US$0.01");
        assert_eq!(format_usd(Cost::from_usd(0.005)), "US$0.01");
        assert_eq!(format_usd(Cost::from_usd(1234.567)), "US$1,234.57");
        assert_eq!(format_usd_fine(0.02112), "US$0.0211");
    }

    #[test]
    fn formats_counts_with_thousands_separators() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(1000), "1,000");
        assert_eq!(format_count(1234567), "1,234,567");
    }
}
