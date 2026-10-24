//! Daily valuation snapshots (spec §4.4): `<data_dir>/<portfolio_id>/snapshots.jsonl`,
//! one JSON object per line and per day. Written only by `portfolio_rs
//! snapshot` (atomic replace, mode 0600); the MCP server only reads them.

use super::analytics;
use crate::portfolio::Portfolio;
use eyre::{Result, WrapErr};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotPosition {
    pub position_id: String,
    /// cash | market | manual
    pub kind: String,
    pub asset_class: String,
    pub quantity: f64,
    #[serde(default)]
    pub price: Option<f64>,
    #[serde(default)]
    pub price_currency: Option<String>,
    #[serde(default)]
    pub fx_rate: Option<f64>,
    /// null when the position had no price or FX rate that day.
    #[serde(default)]
    pub value_base: Option<f64>,
    /// Net buys minus sells (base currency) dated on or before the snapshot
    /// date, as the purchase list said when the snapshot was taken (Finance
    /// B8). Lets `get_performance` notice a ledger edited afterwards.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ledger_base: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    /// YYYY-MM-DD (local date of the valuation).
    pub date: String,
    pub base_currency: String,
    pub positions: Vec<SnapshotPosition>,
    pub cash_base: f64,
    /// Sum of the known values; check `complete`.
    pub total_base: f64,
    /// Always 0 for now: flows come from the purchases ledger at read time.
    #[serde(default)]
    pub net_flows_base: f64,
    /// Every position had a value.
    #[serde(default = "yes")]
    pub complete: bool,
    /// complete | partial | stale
    #[serde(default = "complete_str")]
    pub data_status: String,
}

fn yes() -> bool {
    true
}
fn complete_str() -> String {
    "complete".to_string()
}

/// The snapshot data dir: explicit > `PORTFOLIO_RS_DATA_DIR` >
/// `<workspace>/portfolio/history` > `$XDG_DATA_HOME/portfolio_rs` >
/// `~/.local/share/portfolio_rs`. Never the CWD.
pub fn data_dir(explicit: Option<&Path>, workspace: Option<&Path>) -> Option<PathBuf> {
    if let Some(d) = explicit {
        return Some(d.to_path_buf());
    }
    if let Some(d) = std::env::var_os("PORTFOLIO_RS_DATA_DIR").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(d));
    }
    if let Some(ws) = workspace {
        return Some(ws.join("portfolio").join("history"));
    }
    if let Some(d) = std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(d).join("portfolio_rs"));
    }
    std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .map(|h| PathBuf::from(h).join(".local/share/portfolio_rs"))
}

pub fn snapshots_path(data_dir: &Path, portfolio_id: &str) -> PathBuf {
    data_dir.join(portfolio_id).join("snapshots.jsonl")
}

/// Read all snapshots, sorted by date. A missing file is an empty history;
/// unreadable lines are skipped and counted.
/// Strict `YYYY-MM-DD` (Finance B5): exactly ten ASCII characters, digits
/// and two dashes, and a real calendar day. Rejects `2026-10-17T00:00:00`,
/// `+2026-10-17`, ` 2026-10-17` and `2026-1-7`.
pub fn strict_date(s: &str) -> Option<chrono::NaiveDate> {
    let b = s.as_bytes();
    let shape = b.len() == 10
        && b.iter().enumerate().all(|(i, c)| {
            if i == 4 || i == 7 {
                *c == b'-'
            } else {
                c.is_ascii_digit()
            }
        });
    if !shape {
        return None;
    }
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
}

/// What `upsert` did with the new line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Upsert {
    Written,
    /// The new snapshot was incomplete and a complete one for the same day
    /// already exists, so the file was left alone (Finance B4).
    KeptComplete,
}

/// A snapshot line left out by [`read`], with the reason.
#[derive(Debug, Clone, PartialEq)]
pub struct SkippedLine {
    /// 1-based line number in snapshots.jsonl.
    pub line: usize,
    /// bad_json | bad_date | total_base_null | incomplete_line |
    /// negative_value | total_mismatch
    pub reason: &'static str,
    /// The line's date when it could be read.
    pub date: Option<String>,
}

/// Largest accepted gap between `totalBase` and the sum of the positions'
/// `valueBase` (rounding to cents per position).
pub const TOTAL_TOLERANCE: f64 = 0.05;

/// Parse every line; lines that are not a snapshot with a strict date are
/// returned as skipped. No plausibility checks (see [`validate`]).
fn parse(text: &str) -> (Vec<(usize, Snapshot)>, Vec<SkippedLine>) {
    let mut ok = Vec::new();
    let mut bad = Vec::new();
    for (i, l) in text.lines().enumerate() {
        if l.trim().is_empty() {
            continue;
        }
        let line = i + 1;
        let skip = |reason, date: Option<String>| SkippedLine { line, reason, date };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(l) else {
            bad.push(skip("bad_json", None));
            continue;
        };
        let date = v.get("date").and_then(|d| d.as_str()).map(String::from);
        match serde_json::from_value::<Snapshot>(v.clone()) {
            Ok(s) if strict_date(&s.date).is_some() => ok.push((line, s)),
            Ok(_) => bad.push(skip("bad_date", date)),
            Err(_) if v.get("totalBase").is_some_and(|t| t.is_null()) => {
                bad.push(skip("total_base_null", date))
            }
            Err(_) if date.as_deref().and_then(strict_date).is_none() => {
                bad.push(skip("bad_date", date))
            }
            Err(_) => bad.push(skip("incomplete_line", date)),
        }
    }
    (ok, bad)
}

/// Why a parsed snapshot is implausible (Finance N2): a negative position
/// value, or a `totalBase` that is not the sum of the position values.
pub fn validate(s: &Snapshot) -> Option<&'static str> {
    if s.positions
        .iter()
        .any(|p| p.value_base.is_some_and(|v| v < 0.0 || !v.is_finite()))
        || !s.total_base.is_finite()
    {
        return Some("negative_value");
    }
    let sum: f64 = s.positions.iter().filter_map(|p| p.value_base).sum();
    ((s.total_base - sum).abs() > TOTAL_TOLERANCE).then_some("total_mismatch")
}

fn sort_dedup(mut out: Vec<Snapshot>) -> Vec<Snapshot> {
    out.sort_by(|a, b| a.date.cmp(&b.date));
    out.dedup_by(|b, a| {
        // Keep the later line for a date (it replaced the earlier one).
        if a.date == b.date {
            *a = b.clone();
            true
        } else {
            false
        }
    });
    out
}

/// Snapshots sorted by date (one per day, the last line wins) and the
/// skipped lines: unreadable (bad JSON, a missing field such as a null
/// `totalBase`, a non-strict date) or implausible ([`validate`]).
pub fn read(path: &Path) -> (Vec<Snapshot>, Vec<SkippedLine>) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return (Vec::new(), Vec::new());
    };
    let (parsed, mut bad) = parse(&text);
    let mut out = Vec::new();
    for (line, s) in parsed {
        match validate(&s) {
            Some(reason) => bad.push(SkippedLine {
                line,
                reason,
                date: Some(s.date.clone()),
            }),
            None => out.push(s),
        }
    }
    bad.sort_by_key(|b| b.line);
    (sort_dedup(out), bad)
}

/// Build today's (or `date`'s) snapshot from a valued portfolio.
pub fn from_portfolio(portfolio: &Portfolio, base: &str, date: &str, stale: bool) -> Snapshot {
    let (rows, _, _) =
        analytics::position_rows(portfolio, base, true, None, super::dto::SortBy::Name);
    let txs = analytics::transactions(portfolio, base, None);
    // Cash quantity is the account balance (`Amount`), not a purchase sum.
    let cash_qty: std::collections::HashMap<String, f64> = portfolio
        .positions
        .iter()
        .filter(|p| p.is_cash())
        .map(|p| {
            (
                p.id().unwrap_or_default().to_string(),
                super::dto::qty(p.balance_dec()),
            )
        })
        .collect();
    let positions: Vec<SnapshotPosition> = rows
        .into_iter()
        .map(|r| {
            let quantity = if r.kind == "cash" {
                cash_qty.get(&r.id).copied().unwrap_or(r.quantity)
            } else {
                r.quantity
            };
            SnapshotPosition {
                ledger_base: (r.kind != "cash")
                    .then(|| super::performance::ledger_to_date(&txs, &r.id, date, r.fx_rate))
                    .flatten(),
                position_id: r.id,
                kind: r.kind,
                asset_class: r.asset_class,
                quantity,
                price: r.price,
                price_currency: r.price_currency,
                fx_rate: r.fx_rate,
                value_base: r.value_base,
            }
        })
        .collect();
    let complete = positions.iter().all(|p| p.value_base.is_some());
    let cash_base = round2(
        positions
            .iter()
            .filter(|p| p.kind == "cash")
            .filter_map(|p| p.value_base)
            .sum(),
    );
    let total_base = round2(positions.iter().filter_map(|p| p.value_base).sum());
    Snapshot {
        date: date.to_string(),
        base_currency: base.to_string(),
        positions,
        cash_base,
        total_base,
        net_flows_base: 0.0,
        complete,
        data_status: analytics::data_status(complete, stale).to_string(),
    }
}

pub fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// Insert or replace the snapshot for its date and rewrite the file
/// atomically (temp file in the same dir, fsync, rename, dir fsync), mode 0600.
pub fn upsert(path: &Path, snap: &Snapshot) -> Result<Upsert> {
    let dir = path
        .parent()
        .ok_or_else(|| eyre::eyre!("bad snapshot path"))?;
    // Directories this call creates (the data dir and its parents too) get
    // 0700; existing ones keep their mode, except the per-portfolio dir.
    let created: Vec<PathBuf> = dir
        .ancestors()
        .take_while(|d| !d.as_os_str().is_empty() && !d.exists())
        .map(Path::to_path_buf)
        .collect();
    std::fs::create_dir_all(dir).wrap_err("cannot create the snapshot directory")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for d in created
            .iter()
            .map(PathBuf::as_path)
            .chain(std::iter::once(dir))
        {
            let _ = std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700));
        }
    }
    // Keep implausible lines as they are (they are only skipped on read).
    let mut all = sort_dedup(
        std::fs::read_to_string(path)
            .map(|t| parse(&t).0.into_iter().map(|(_, s)| s).collect())
            .unwrap_or_default(),
    );
    if !snap.complete && all.iter().any(|s| s.date == snap.date && s.complete) {
        return Ok(Upsert::KeptComplete);
    }
    all.retain(|s| s.date != snap.date);
    all.push(snap.clone());
    all.sort_by(|a, b| a.date.cmp(&b.date));
    let mut body = String::new();
    for s in &all {
        body.push_str(&serde_json::to_string(s)?);
        body.push('\n');
    }
    let tmp = dir.join(format!(".snapshots.jsonl.tmp-{}", std::process::id()));
    {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts
            .open(&tmp)
            .wrap_err("cannot create the snapshot temp file")?;
        f.write_all(body.as_bytes())?;
        f.sync_all()?;
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e).wrap_err("cannot replace the snapshot file");
    }
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(Upsert::Written)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(date: &str, total: f64) -> Snapshot {
        Snapshot {
            date: date.into(),
            base_currency: "EUR".into(),
            positions: vec![SnapshotPosition {
                position_id: "pos_cash".into(),
                kind: "cash".into(),
                asset_class: "Cash".into(),
                quantity: total,
                price: None,
                price_currency: None,
                fx_rate: None,
                value_base: Some(total),
                ledger_base: None,
            }],
            cash_base: total,
            total_base: total,
            net_flows_base: 0.0,
            complete: true,
            data_status: "complete".into(),
        }
    }

    #[cfg(unix)]
    #[test]
    fn upsert_creates_private_parents() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("a/b");
        upsert(&snapshots_path(&data, "main"), &snap("2026-01-01", 1.0)).unwrap();
        for d in [dir.path().join("a"), data.clone(), data.join("main")] {
            let mode = std::fs::metadata(&d).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700, "{}", d.display());
        }
    }

    #[test]
    fn strict_date_rejects_loose_forms() {
        assert!(strict_date("2026-10-17").is_some());
        for bad in [
            "2026-10-17T00:00:00",
            "+2026-10-17",
            " 2026-10-17",
            "2026-1-7",
            "2026-02-30",
            "20261017",
            "",
        ] {
            assert!(strict_date(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn read_skips_non_strict_dates() {
        let dir = tempfile::tempdir().unwrap();
        let path = snapshots_path(dir.path(), "main");
        upsert(&path, &snap("2026-01-01", 1.0)).unwrap();
        let mut loose = serde_json::to_value(snap("2026-01-02", 2.0)).unwrap();
        loose["date"] = "2026-01-02T00:00:00".into();
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str(&format!("{loose}\nnot json\n"));
        std::fs::write(&path, text).unwrap();
        let (all, bad) = read(&path);
        assert_eq!(bad.len(), 2);
        assert_eq!(bad[0].reason, "bad_date");
        assert_eq!(bad[1].reason, "bad_json");
        assert_eq!(all.len(), 1);
    }

    /// Finance N2: negative values and a total off the positions' sum.
    #[test]
    fn validate_flags_implausible_snapshots() {
        let mut s = snap("2026-01-01", 10.0);
        assert_eq!(validate(&s), None);
        s.total_base = 10.04;
        assert_eq!(validate(&s), None);
        s.total_base = 11.0;
        assert_eq!(validate(&s), Some("total_mismatch"));
        s.positions[0].value_base = Some(-11.0);
        assert_eq!(validate(&s), Some("negative_value"));
    }

    /// Finance B4: an incomplete snapshot never replaces a complete one.
    #[test]
    fn incomplete_snapshot_keeps_same_day_complete_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = snapshots_path(dir.path(), "main");
        assert_eq!(
            upsert(&path, &snap("2026-01-02", 1.0)).unwrap(),
            Upsert::Written
        );
        let mut partial = snap("2026-01-02", 0.5);
        partial.complete = false;
        partial.data_status = "partial".into();
        assert_eq!(upsert(&path, &partial).unwrap(), Upsert::KeptComplete);
        let (all, _) = read(&path);
        assert_eq!(all[0].total_base, 1.0);
        assert!(all[0].complete);
        // A complete one still replaces; an incomplete one replaces an incomplete one.
        assert_eq!(
            upsert(&path, &snap("2026-01-02", 2.0)).unwrap(),
            Upsert::Written
        );
        let mut p2 = partial.clone();
        p2.date = "2026-01-03".into();
        upsert(&path, &p2).unwrap();
        p2.total_base = 0.7;
        p2.positions[0].value_base = Some(0.7);
        assert_eq!(upsert(&path, &p2).unwrap(), Upsert::Written);
        let (all, _) = read(&path);
        assert_eq!(
            all.iter().map(|s| s.total_base).collect::<Vec<_>>(),
            vec![2.0, 0.7]
        );
    }

    #[test]
    fn upsert_replaces_same_day_and_is_private() {
        let dir = tempfile::tempdir().unwrap();
        let path = snapshots_path(dir.path(), "main");
        upsert(&path, &snap("2026-01-02", 1.0)).unwrap();
        upsert(&path, &snap("2026-01-01", 2.0)).unwrap();
        upsert(&path, &snap("2026-01-02", 3.0)).unwrap();
        let (all, bad) = read(&path);
        assert!(bad.is_empty());
        assert_eq!(
            all.iter().map(|s| s.total_base).collect::<Vec<_>>(),
            vec![2.0, 3.0]
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        // No temp files left behind.
        let names: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 1, "{names:?}");
    }

    #[test]
    fn data_dir_prefers_explicit_then_workspace() {
        let ws = Path::new("/ws");
        assert_eq!(
            data_dir(Some(Path::new("/x")), Some(ws)).unwrap(),
            PathBuf::from("/x")
        );
        if std::env::var_os("PORTFOLIO_RS_DATA_DIR").is_none() {
            assert_eq!(
                data_dir(None, Some(ws)).unwrap(),
                PathBuf::from("/ws/portfolio/history")
            );
        }
    }
}
