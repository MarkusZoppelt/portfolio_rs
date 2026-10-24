//! Decimal money math.
//!
//! Stored data and DTO/HTTP shapes stay `f64`; values are converted to
//! [`Decimal`] at the start of a calculation and back to `f64` only at that
//! boundary, so sums and products of prices, quantities and FX rates do not
//! accumulate binary floating-point error.

use rust_decimal::prelude::{FromPrimitive, ToPrimitive};
pub use rust_decimal::Decimal;

/// `f64` -> [`Decimal`]. Non-finite or out-of-range input becomes zero.
pub fn dec(value: f64) -> Decimal {
    Decimal::from_f64(value).unwrap_or(Decimal::ZERO)
}

/// [`Decimal`] -> `f64`, for DTOs, display and other f64 boundaries.
pub fn to_f64(value: Decimal) -> f64 {
    value.to_f64().unwrap_or(0.0)
}

/// `a / b`, or `None` when `b` is zero or the result overflows.
pub fn div(a: Decimal, b: Decimal) -> Option<Decimal> {
    a.checked_div(b)
}

/// Percentage change `(current - base) / base * 100`; `None` unless `base > 0`.
pub fn percent_change(current: Decimal, base: Decimal) -> Option<Decimal> {
    if base <= Decimal::ZERO {
        return None;
    }
    div(current - base, base).map(|r| r * Decimal::ONE_HUNDRED)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimal_sums_are_exact() {
        let total: Decimal = [0.1, 0.2].into_iter().map(dec).sum();
        assert_eq!(total, dec(0.3));
        assert_eq!(to_f64(total), 0.3);
        // The f64 sum is not.
        assert_ne!(0.1_f64 + 0.2, 0.3);
    }

    #[test]
    fn non_finite_and_division_by_zero_are_safe() {
        assert_eq!(dec(f64::NAN), Decimal::ZERO);
        assert_eq!(dec(f64::INFINITY), Decimal::ZERO);
        assert_eq!(div(Decimal::ONE, Decimal::ZERO), None);
        assert_eq!(percent_change(dec(110.0), Decimal::ZERO), None);
        assert_eq!(percent_change(dec(110.0), dec(100.0)), Some(dec(10.0)));
    }
}
