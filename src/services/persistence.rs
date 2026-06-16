use crate::portfolio::Portfolio;
use eyre::{Result, WrapErr};

pub fn store_balance_in_db(portfolio: &Portfolio) -> Result<()> {
    let db = sled::open("database").wrap_err("failed to open database")?;
    let curr_value = portfolio.get_total_value();
    let curr_time = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();

    db.insert(&curr_time, curr_value.to_string().as_bytes())
        .wrap_err_with(|| format!("failed to insert balance into database at {}", curr_time))?;

    // block until all operations are stable on disk
    db.flush().wrap_err("failed to flush database to disk")?;

    Ok(())
}
