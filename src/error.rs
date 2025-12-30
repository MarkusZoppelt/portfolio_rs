//! Error types for portfolio_rs
//!
//! This module defines domain-specific error types that provide clear,
//! actionable error messages to users.

use thiserror::Error;

/// Validation errors for user input in the TUI.
///
/// These errors are shown directly to users and should be clear and actionable.
#[derive(Debug, Error)]
pub enum ValidationError {
    #[error("Date is required")]
    DateRequired,

    #[error("Quantity is required")]
    QuantityRequired,

    #[error("Invalid quantity format: {0}")]
    InvalidQuantity(String),

    #[error("Quantity must be positive, got {0}")]
    NonPositiveQuantity(f64),

    #[error("Invalid price format: {0}")]
    InvalidPrice(String),

    #[error("Price cannot be negative, got {0}")]
    NegativePrice(f64),
}
