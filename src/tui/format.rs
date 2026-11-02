//! Number and currency formatting for the TUI.

/// Insert a `,` every three digits from the right of `digits`.
fn group_thousands(digits: &str) -> String {
    digits
        .chars()
        .rev()
        .collect::<Vec<_>>()
        .chunks(3)
        .map(|chunk| chunk.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join(",")
        .chars()
        .rev()
        .collect()
}

/// `value` as a whole number with thousands separators.
fn format_whole(value: f64) -> String {
    group_thousands(&format!("{}", value as i64))
}

pub(super) fn format_currency(value: f64, currency: &str) -> String {
    let formatted_number = if value >= 1000.0 {
        format_with_commas(value)
    } else {
        match currency {
            "JPY" => format!("{value:.0}"),
            _ => format!("{value:.2}"),
        }
    };

    match currency {
        "USD" | "CAD" | "AUD" | "HKD" | "SGD" => format!("${formatted_number}"),
        "EUR" => format!("{formatted_number} €"),
        "GBP" => format!("£{formatted_number}"),
        "JPY" => format!("{} JPY", format_whole(value)),
        _ => format!("{formatted_number} {currency}"),
    }
}

/// The total shown in big text on the overview tab.
pub(super) fn format_big_value(value: f64, currency: &str) -> String {
    match currency {
        "USD" | "CAD" | "AUD" | "HKD" | "SGD" => format!("${}", format_with_commas(value)),
        "GBP" => format!("£{}", format_with_commas(value)),
        "JPY" => format!("{} JPY", format_whole(value)),
        _ => format!("{} {currency}", format_with_commas(value)),
    }
}

pub(super) fn format_with_commas(value: f64) -> String {
    let formatted = format!("{value:.2}");
    // `{:.2}` always yields `<integer>.<two digits>`.
    let (integer_part, decimal_part) = formatted.split_once('.').unwrap_or((&formatted, "00"));
    format!("{}.{decimal_part}", group_thousands(integer_part))
}

pub(super) fn format_amount(amount: f64) -> String {
    if amount.fract() == 0.0 {
        format!("{amount:.0}")
    } else if amount >= 1.0 {
        format!("{amount:.2}")
    } else if amount >= 0.01 {
        format!("{amount:.4}")
    } else {
        format!("{amount:.8}")
    }
}
