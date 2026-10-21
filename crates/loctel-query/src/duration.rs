//! Duration literals: `500ms`, `1.5s`, `1h30m`.

/// Parses a duration literal into nanoseconds.
///
/// Units are `ns`, `us`, `µs` (micro sign or Greek mu), `ms`, `s`, `m` and
/// `h`. Decimals (`1.5s`) and compound forms (`1h30m`) are allowed, and every
/// number needs its unit. Anything else, including a value too big for `u64`
/// nanoseconds, is `None`.
pub fn parse_duration(text: &str) -> Option<u64> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return None;
    }
    let mut total: u128 = 0;
    let mut i = 0;
    while i < chars.len() {
        let int_start = i;
        while i < chars.len() && chars[i].is_ascii_digit() {
            i += 1;
        }
        let int_digits: String = chars[int_start..i].iter().collect();

        let mut frac_digits = String::new();
        if chars.get(i) == Some(&'.') {
            i += 1;
            let frac_start = i;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            frac_digits = chars[frac_start..i].iter().collect();
        }
        if int_digits.is_empty() && frac_digits.is_empty() {
            return None;
        }

        let unit_start = i;
        while i < chars.len() && !chars[i].is_ascii_digit() && chars[i] != '.' {
            i += 1;
        }
        let unit: String = chars[unit_start..i].iter().collect();
        let nanos_per_unit: u128 = match unit.as_str() {
            "ns" => 1,
            "us" | "µs" | "μs" => 1_000,
            "ms" => 1_000_000,
            "s" => 1_000_000_000,
            "m" => 60 * 1_000_000_000,
            "h" => 3_600 * 1_000_000_000,
            _ => return None,
        };

        let whole: u128 = if int_digits.is_empty() {
            0
        } else {
            int_digits.parse().ok()?
        };
        total = total.checked_add(whole.checked_mul(nanos_per_unit)?)?;

        if !frac_digits.is_empty() {
            // Past 18 digits the fraction is far below a nanosecond for every
            // unit, so the rest is dropped (this also keeps the maths in u128).
            let kept: String = frac_digits.chars().take(18).collect();
            let numerator: u128 = kept.parse().ok()?;
            let denominator = 10u128.pow(kept.len() as u32);
            total = total.checked_add(numerator.checked_mul(nanos_per_unit)? / denominator)?;
        }
    }
    u64::try_from(total).ok()
}
