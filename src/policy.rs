use eyre::{Result, WrapErr};
use serde::{Deserialize, Serialize};
use std::{
    fmt::{Display, Formatter},
    fs,
};

pub fn default_balanced_growth_policy() -> Policy {
    Policy {
        version: CURRENT_POLICY_VERSION.to_string(),
        name: "Balanced Growth".to_string(),
        base_currency: "EUR".to_string(),
        time_horizon_years: 15,
        risk_profile: RiskProfile::Moderate,
        constraints: Constraints {
            minimum_cash_months: Some(6),
            minimum_cash_amount: Some(30000.0),
            single_position_limit_percent: Some(25.0),
            asset_class_limit_percent: Some(60.0),
        },
        allocations: vec![
            AllocationTarget {
                asset_class: "Stocks".to_string(),
                target_percent: 55.0,
                tolerance_percent: Some(10.0),
            },
            AllocationTarget {
                asset_class: "Bonds".to_string(),
                target_percent: 25.0,
                tolerance_percent: Some(5.0),
            },
            AllocationTarget {
                asset_class: "Cash".to_string(),
                target_percent: 10.0,
                tolerance_percent: Some(5.0),
            },
            AllocationTarget {
                asset_class: "Crypto".to_string(),
                target_percent: 5.0,
                tolerance_percent: Some(3.0),
            },
            AllocationTarget {
                asset_class: "Gold".to_string(),
                target_percent: 5.0,
                tolerance_percent: Some(3.0),
            },
        ],
    }
}

pub fn default_capital_preservation_policy() -> Policy {
    Policy {
        version: CURRENT_POLICY_VERSION.to_string(),
        name: "Capital Preservation".to_string(),
        base_currency: "EUR".to_string(),
        time_horizon_years: 5,
        risk_profile: RiskProfile::Conservative,
        constraints: Constraints {
            minimum_cash_months: Some(12),
            minimum_cash_amount: Some(50000.0),
            single_position_limit_percent: Some(15.0),
            asset_class_limit_percent: Some(40.0),
        },
        allocations: vec![
            AllocationTarget {
                asset_class: "Bonds".to_string(),
                target_percent: 50.0,
                tolerance_percent: Some(5.0),
            },
            AllocationTarget {
                asset_class: "Cash".to_string(),
                target_percent: 30.0,
                tolerance_percent: Some(5.0),
            },
            AllocationTarget {
                asset_class: "Stocks".to_string(),
                target_percent: 15.0,
                tolerance_percent: Some(5.0),
            },
            AllocationTarget {
                asset_class: "Gold".to_string(),
                target_percent: 5.0,
                tolerance_percent: Some(2.0),
            },
        ],
    }
}

pub fn default_aggressive_growth_policy() -> Policy {
    Policy {
        version: CURRENT_POLICY_VERSION.to_string(),
        name: "Aggressive Growth".to_string(),
        base_currency: "EUR".to_string(),
        time_horizon_years: 20,
        risk_profile: RiskProfile::Aggressive,
        constraints: Constraints {
            minimum_cash_months: Some(3),
            minimum_cash_amount: Some(15000.0),
            single_position_limit_percent: Some(30.0),
            asset_class_limit_percent: Some(70.0),
        },
        allocations: vec![
            AllocationTarget {
                asset_class: "Stocks".to_string(),
                target_percent: 70.0,
                tolerance_percent: Some(10.0),
            },
            AllocationTarget {
                asset_class: "Crypto".to_string(),
                target_percent: 10.0,
                tolerance_percent: Some(5.0),
            },
            AllocationTarget {
                asset_class: "Bonds".to_string(),
                target_percent: 10.0,
                tolerance_percent: Some(5.0),
            },
            AllocationTarget {
                asset_class: "Cash".to_string(),
                target_percent: 5.0,
                tolerance_percent: Some(3.0),
            },
            AllocationTarget {
                asset_class: "Gold".to_string(),
                target_percent: 5.0,
                tolerance_percent: Some(3.0),
            },
        ],
    }
}

pub fn policy_from_strategy(strategy: &str) -> Option<Policy> {
    match strategy {
        "balanced-growth" => Some(default_balanced_growth_policy()),
        "capital-preservation" => Some(default_capital_preservation_policy()),
        "aggressive-growth" => Some(default_aggressive_growth_policy()),
        "custom" => Some(Policy {
            version: CURRENT_POLICY_VERSION.to_string(),
            name: "Custom".to_string(),
            base_currency: "EUR".to_string(),
            time_horizon_years: 10,
            risk_profile: RiskProfile::Moderate,
            constraints: Constraints::default(),
            allocations: vec![AllocationTarget {
                asset_class: "Cash".to_string(),
                target_percent: 100.0,
                tolerance_percent: None,
            }],
        }),
        _ => None,
    }
}

pub const CURRENT_POLICY_VERSION: &str = "1.0";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Policy {
    pub version: String,
    pub name: String,
    pub base_currency: String,
    pub time_horizon_years: u32,
    pub risk_profile: RiskProfile,
    #[serde(default)]
    pub constraints: Constraints,
    #[serde(default)]
    pub allocations: Vec<AllocationTarget>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RiskProfile {
    Conservative,
    Moderate,
    Aggressive,
}

impl Display for RiskProfile {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            RiskProfile::Conservative => write!(f, "conservative"),
            RiskProfile::Moderate => write!(f, "moderate"),
            RiskProfile::Aggressive => write!(f, "aggressive"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Constraints {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimum_cash_months: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimum_cash_amount: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub single_position_limit_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asset_class_limit_percent: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AllocationTarget {
    pub asset_class: String,
    pub target_percent: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tolerance_percent: Option<f64>,
}

impl Policy {
    pub fn from_file(path: &str) -> Result<Self> {
        let content = fs::read_to_string(path)
            .wrap_err_with(|| format!("failed to read policy file: {}", path))?;
        content.parse::<Policy>()
    }

    pub fn to_toml(&self) -> Result<String> {
        toml::to_string_pretty(self).wrap_err("failed to serialize policy to TOML")
    }

    pub fn validate(&self) -> Result<()> {
        // Allow small rounding slack (e.g. 33.4 + 33.3 + 33.3).
        const ALLOCATION_SUM_TOLERANCE: f64 = 0.01;
        let total: f64 = self.allocations.iter().map(|a| a.target_percent).sum();
        if (total - 100.0).abs() > ALLOCATION_SUM_TOLERANCE {
            return Err(eyre::eyre!(
                "policy allocation targets must sum to 100%, got {:.2}%",
                total
            ));
        }

        for allocation in &self.allocations {
            if allocation.target_percent < 0.0 {
                return Err(eyre::eyre!(
                    "allocation target for '{}' cannot be negative",
                    allocation.asset_class
                ));
            }
        }

        Ok(())
    }

    pub fn get_allocation(&self, asset_class: &str) -> Option<&AllocationTarget> {
        self.allocations
            .iter()
            .find(|a| a.asset_class.eq_ignore_ascii_case(asset_class))
    }
}

impl std::str::FromStr for Policy {
    type Err = eyre::Report;

    fn from_str(content: &str) -> Result<Self, Self::Err> {
        let policy: Policy = toml::from_str(content).wrap_err("failed to parse policy TOML")?;
        policy.validate()?;
        Ok(policy)
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    #[test]
    fn test_policy_from_str_valid() {
        let toml = r#"
version = "1.0"
name = "Balanced Growth"
base_currency = "EUR"
time_horizon_years = 15
risk_profile = "moderate"

[constraints]
minimum_cash_months = 6
minimum_cash_amount = 30000.0
single_position_limit_percent = 25.0

[[allocations]]
asset_class = "Stocks"
target_percent = 55.0
tolerance_percent = 10.0

[[allocations]]
asset_class = "Bonds"
target_percent = 25.0
tolerance_percent = 5.0

[[allocations]]
asset_class = "Cash"
target_percent = 10.0
tolerance_percent = 5.0

[[allocations]]
asset_class = "Crypto"
target_percent = 5.0
tolerance_percent = 3.0

[[allocations]]
asset_class = "Gold"
target_percent = 5.0
tolerance_percent = 3.0
"#;

        let policy = Policy::from_str(toml).unwrap();
        assert_eq!(policy.version, "1.0");
        assert_eq!(policy.name, "Balanced Growth");
        assert_eq!(policy.base_currency, "EUR");
        assert_eq!(policy.time_horizon_years, 15);
        assert_eq!(policy.risk_profile, RiskProfile::Moderate);
        assert_eq!(policy.constraints.minimum_cash_months, Some(6));
        assert_eq!(policy.allocations.len(), 5);
    }

    #[test]
    fn test_policy_validation_allocation_sum() {
        let toml = r#"
version = "1.0"
name = "Invalid"
base_currency = "EUR"
time_horizon_years = 10
risk_profile = "moderate"

[[allocations]]
asset_class = "Stocks"
target_percent = 50.0

[[allocations]]
asset_class = "Bonds"
target_percent = 30.0
"#;

        let result = Policy::from_str(toml);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("must sum to 100%"));
    }

    #[test]
    fn test_policy_get_allocation() {
        let toml = r#"
version = "1.0"
name = "Test"
base_currency = "EUR"
time_horizon_years = 10
risk_profile = "moderate"

[[allocations]]
asset_class = "Stocks"
target_percent = 60.0

[[allocations]]
asset_class = "Bonds"
target_percent = 40.0
"#;

        let policy = Policy::from_str(toml).unwrap();
        let stocks = policy.get_allocation("Stocks");
        assert!(stocks.is_some());
        assert_eq!(stocks.unwrap().target_percent, 60.0);
        assert!(policy.get_allocation("Missing").is_none());
    }

    #[test]
    fn test_policy_roundtrip() {
        let toml = r#"
version = "1.0"
name = "Test"
base_currency = "USD"
time_horizon_years = 5
risk_profile = "conservative"

[constraints]
single_position_limit_percent = 20.0

[[allocations]]
asset_class = "Cash"
target_percent = 100.0
"#;

        let policy = Policy::from_str(toml).unwrap();
        let serialized = policy.to_toml().unwrap();
        let reparsed = Policy::from_str(&serialized).unwrap();
        assert_eq!(policy.name, reparsed.name);
        assert_eq!(policy.base_currency, reparsed.base_currency);
        assert_eq!(policy.allocations.len(), reparsed.allocations.len());
    }
}
