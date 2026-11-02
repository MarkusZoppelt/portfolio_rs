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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn currency_symbols_and_separators() {
        assert_eq!(format_currency(1234.5, "USD"), "$1,234.50");
        assert_eq!(format_currency(12.345, "CAD"), "$12.35");
        assert_eq!(format_currency(1234567.0, "EUR"), "1,234,567.00 €");
        assert_eq!(format_currency(5.0, "GBP"), "£5.00");
        assert_eq!(format_currency(999.6, "CHF"), "999.60 CHF");
        assert_eq!(format_currency(2500.0, "SEK"), "2,500.00 SEK");
    }

    #[test]
    fn yen_has_no_decimals() {
        assert_eq!(format_currency(1234567.89, "JPY"), "1,234,567 JPY");
        assert_eq!(format_currency(999.0, "JPY"), "999 JPY");
    }

    #[test]
    fn big_value_always_groups_thousands() {
        assert_eq!(format_big_value(1234.5, "HKD"), "$1,234.50");
        assert_eq!(format_big_value(1234.5, "GBP"), "£1,234.50");
        assert_eq!(format_big_value(1234.5, "EUR"), "1,234.50 EUR");
        assert_eq!(format_big_value(98765.4, "JPY"), "98,765 JPY");
        assert_eq!(format_big_value(12.0, "CHF"), "12.00 CHF");
    }

    #[test]
    fn commas_group_the_integer_part() {
        assert_eq!(format_with_commas(0.0), "0.00");
        assert_eq!(format_with_commas(123.456), "123.46");
        assert_eq!(format_with_commas(1000.0), "1,000.00");
        assert_eq!(format_with_commas(9876543.21), "9,876,543.21");
        // Not a number has no decimal part to split off.
        assert_eq!(format_with_commas(f64::NAN), "NaN.00");
    }

    #[test]
    fn amounts_show_more_decimals_when_small() {
        assert_eq!(format_amount(50.0), "50");
        assert_eq!(format_amount(12.345), "12.35");
        assert_eq!(format_amount(0.12345), "0.1235");
        assert_eq!(format_amount(0.000123456), "0.00012346");
    }
}
