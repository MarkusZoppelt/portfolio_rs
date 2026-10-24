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
pub fn read(path: &Path) -> (Vec<Snapshot>, usize) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return (Vec::new(), 0);
    };
    let mut bad = 0;
    let mut out: Vec<Snapshot> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| match serde_json::from_str(l) {
            Ok(s) => Some(s),
            Err(_) => {
                bad += 1;
                None
            }
        })
        .collect();
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
    (out, bad)
}

/// Build today's (or `date`'s) snapshot from a valued portfolio.
pub fn from_portfolio(portfolio: &Portfolio, base: &str, date: &str, stale: bool) -> Snapshot {
    let (rows, _, _) =
        analytics::position_rows(portfolio, base, true, None, super::dto::SortBy::Name);
    let positions: Vec<SnapshotPosition> = rows
        .into_iter()
        .map(|r| SnapshotPosition {
            position_id: r.id,
            kind: r.kind,
            asset_class: r.asset_class,
            quantity: r.quantity,
            price: r.price,
            price_currency: r.price_currency,
            fx_rate: r.fx_rate,
            value_base: r.value_base,
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
pub fn upsert(path: &Path, snap: &Snapshot) -> Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| eyre::eyre!("bad snapshot path"))?;
    std::fs::create_dir_all(dir).wrap_err("cannot create the snapshot directory")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    let (mut all, _) = read(path);
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
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(date: &str, total: f64) -> Snapshot {
        Snapshot {
            date: date.into(),
            base_currency: "EUR".into(),
            positions: vec![],
            cash_base: 0.0,
            total_base: total,
            net_flows_base: 0.0,
            complete: true,
            data_status: "complete".into(),
        }
    }

    #[test]
    fn upsert_replaces_same_day_and_is_private() {
        let dir = tempfile::tempdir().unwrap();
        let path = snapshots_path(dir.path(), "main");
        upsert(&path, &snap("2026-01-02", 1.0)).unwrap();
        upsert(&path, &snap("2026-01-01", 2.0)).unwrap();
        upsert(&path, &snap("2026-01-02", 3.0)).unwrap();
        let (all, bad) = read(&path);
        assert_eq!(bad, 0);
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
