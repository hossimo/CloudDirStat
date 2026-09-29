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

/// Dollars with cents, e.g. `$1,234.56`. Tiny nonzero amounts show as `<$0.01`.
pub fn format_usd(cost: Cost) -> String {
    let cents = (cost.usd() * 100.0).round() as u64;
    if cents == 0 && cost > Cost::ZERO {
        return "<$0.01".to_owned();
    }
    format!("${}.{:02}", format_count(cents / 100), cents % 100)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(format_usd(Cost::ZERO), "$0.00");
        assert_eq!(format_usd(Cost::from_usd(0.001)), "<$0.01");
        assert_eq!(format_usd(Cost::from_usd(0.005)), "$0.01");
        assert_eq!(format_usd(Cost::from_usd(1234.567)), "$1,234.57");
    }

    #[test]
    fn formats_counts_with_thousands_separators() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(1000), "1,000");
        assert_eq!(format_count(1234567), "1,234,567");
    }
}
