//! TUI state and the purchase edits it writes back to the portfolio file.

use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use eyre::{bail, eyre, Result, WrapErr};
use ratatui::style::Color;
use serde_json::{json, Map, Number, Value};
use tokio::sync::mpsc;

use super::{AppMode, DisabledComponents, EditField, NetworkStatus, Tab, Trend};
use crate::error::ValidationError;
use crate::money::to_f64;
use crate::portfolio::Portfolio;
use crate::position::{ensure_json_position_ids, PortfolioPosition, Purchase};
use crate::services::atomic_write::write_atomic_with_backup;

pub struct App {
    pub current_tab: Tab,
    pub portfolio: Option<Portfolio>,
    pub should_quit: bool,
    pub loading: bool,
    pub error_message: Option<String>,
    pub currency: String,
    pub previous_values: HashMap<String, f64>,
    pub trends: HashMap<String, Trend>,
    pub last_update: Instant,
    pub flash_state: bool,
    pub positions_str: String,
    pub mode: AppMode,
    pub selected_position: usize,
    pub selected_purchase: usize,
    pub edit_input: String,
    pub purchase_date_input: String,
    pub purchase_quantity_input: String,
    pub purchase_price_input: String,
    pub edit_field: EditField,
    pub data_file_path: String,
    pub portfolio_receiver: Option<mpsc::UnboundedReceiver<(Portfolio, NetworkStatus)>>,
    // Historic graph data and channel to receive async updates
    pub historic_data: Option<Vec<(f64, f64)>>,
    pub historic_receiver: Option<mpsc::UnboundedReceiver<Vec<(f64, f64)>>>,
    pub network_status: NetworkStatus,
    pub disabled_components: DisabledComponents,
}

impl App {
    pub fn new(
        currency: String,
        positions_str: String,
        data_file_path: String,
        disabled_components: DisabledComponents,
    ) -> App {
        App {
            current_tab: Tab::Overview,
            portfolio: None,
            should_quit: false,
            loading: false,
            error_message: None,
            currency,
            previous_values: HashMap::new(),
            trends: HashMap::new(),
            last_update: Instant::now(),
            flash_state: false,
            positions_str,
            mode: AppMode::Normal,
            selected_position: 0,
            selected_purchase: 0,
            edit_input: String::new(),
            purchase_date_input: String::new(),
            purchase_quantity_input: String::new(),
            purchase_price_input: String::new(),
            edit_field: EditField::Date,
            data_file_path,
            portfolio_receiver: None,
            historic_data: None,
            historic_receiver: None,
            network_status: NetworkStatus::Connected,
            disabled_components,
        }
    }

    pub fn set_portfolio_receiver(
        &mut self,
        receiver: mpsc::UnboundedReceiver<(Portfolio, NetworkStatus)>,
    ) {
        self.portfolio_receiver = Some(receiver);
    }

    pub fn set_historic_receiver(&mut self, receiver: mpsc::UnboundedReceiver<Vec<(f64, f64)>>) {
        self.historic_receiver = Some(receiver);
    }

    pub fn try_receive_portfolio_update(&mut self) -> bool {
        let Some(Ok((portfolio, network_status))) =
            self.portfolio_receiver.as_mut().map(|r| r.try_recv())
        else {
            return false;
        };
        // Portfolio is already sorted by the background task
        self.apply_refresh(portfolio, network_status);
        true
    }

    pub fn try_receive_historic_update(&mut self) -> bool {
        let Some(Ok(series)) = self.historic_receiver.as_mut().map(|r| r.try_recv()) else {
            return false;
        };
        self.historic_data = Some(series);
        self.mark_refreshed();
        true
    }

    /// Show a freshly valued portfolio and flash the refresh indicator.
    pub(super) fn apply_refresh(&mut self, portfolio: Portfolio, network_status: NetworkStatus) {
        self.update_trends(&portfolio);
        self.set_portfolio(portfolio);
        self.network_status = network_status;
        self.mark_refreshed();
    }

    pub fn set_portfolio(&mut self, portfolio: Portfolio) {
        self.portfolio = Some(portfolio);
        self.loading = false;
    }

    fn tab_index(&self) -> usize {
        Tab::all()
            .iter()
            .position(|&t| t == self.current_tab)
            .unwrap_or(0)
    }

    pub fn next_tab(&mut self) {
        let tabs = Tab::all();
        self.current_tab = tabs[(self.tab_index() + 1) % tabs.len()];
    }

    pub fn previous_tab(&mut self) {
        let tabs = Tab::all();
        self.current_tab = tabs[(self.tab_index() + tabs.len() - 1) % tabs.len()];
    }

    pub fn update_trends(&mut self, portfolio: &Portfolio) {
        // Use a small threshold to avoid noise from tiny changes
        let threshold = 0.01; // 1 cent threshold
        for position in &portfolio.positions {
            let name = position.get_name().to_string();
            let current_value = position.get_balance();

            let trend = match self.previous_values.get(&name) {
                Some(&previous) if current_value > previous + threshold => Trend::Up,
                Some(&previous) if current_value < previous - threshold => Trend::Down,
                // Keep the previous trend if the change is too small
                Some(_) => self.trends.get(&name).copied().unwrap_or(Trend::Neutral),
                // First time seeing this position
                None => Trend::Neutral,
            };
            self.trends.insert(name.clone(), trend);
            self.previous_values.insert(name, current_value);
        }
    }

    pub fn mark_refreshed(&mut self) {
        self.last_update = Instant::now();
        self.flash_state = !self.flash_state; // Toggle flash state for animation
    }

    pub fn get_trend_color(&self, name: &str, base_color: Color) -> Color {
        match (self.trends.get(name), self.flash_state) {
            (Some(Trend::Up), true) => Color::LightGreen,
            (Some(Trend::Up), false) => Color::Green,
            (Some(Trend::Down), true) => Color::LightRed,
            (Some(Trend::Down), false) => Color::Red,
            _ => base_color,
        }
    }

    pub fn select_next(&mut self) {
        if let Some(portfolio) = &self.portfolio {
            if self.selected_position < portfolio.positions.len().saturating_sub(1) {
                self.selected_position += 1;
            }
        }
    }

    pub fn select_previous(&mut self) {
        self.selected_position = self.selected_position.saturating_sub(1);
    }

    pub fn enter_edit_mode(&mut self) {
        if self.selected_portfolio_position().is_some() {
            self.mode = AppMode::PurchaseList;
            self.selected_purchase = 0;
        }
    }

    pub fn exit_edit_mode(&mut self) {
        self.mode = AppMode::Normal;
        self.edit_input.clear();
        self.clear_purchase_inputs();
    }

    /// Empty the date, quantity and price inputs of the purchase form.
    pub(super) fn clear_purchase_inputs(&mut self) {
        self.purchase_date_input.clear();
        self.purchase_quantity_input.clear();
        self.purchase_price_input.clear();
    }

    pub fn select_next_purchase(&mut self) {
        if let Some(position) = self.selected_portfolio_position() {
            // 0 = Add New, 1 to purchases.len() = existing purchases
            if self.selected_purchase < position.get_purchases().len() {
                self.selected_purchase += 1;
            }
        }
    }

    pub fn select_previous_purchase(&mut self) {
        self.selected_purchase = self.selected_purchase.saturating_sub(1);
    }

    pub fn enter_add_purchase_mode(&mut self) {
        self.mode = AppMode::AddPurchase;
        self.edit_field = EditField::Date;
        self.clear_purchase_inputs();
    }

    pub fn enter_edit_purchase_mode(&mut self) {
        let Some((original_index, purchase)) = self.selected_purchase_entry() else {
            return;
        };
        let date = purchase.date.clone().unwrap_or_default();
        let quantity = to_f64(purchase.quantity).to_string();
        // Only prefill the price when the original JSON has one, so an
        // automatic price is not saved by accident.
        let price = self.price_in_file(original_index);

        self.mode = AppMode::EditPurchase;
        self.edit_field = EditField::Date;
        self.purchase_date_input = date;
        self.purchase_quantity_input = quantity;
        self.purchase_price_input = price.map(|p| p.to_string()).unwrap_or_default();
    }

    /// The position under the cursor.
    pub(super) fn selected_portfolio_position(&self) -> Option<&PortfolioPosition> {
        self.portfolio
            .as_ref()?
            .positions
            .get(self.selected_position)
    }

    /// `(index in the file, purchase)` under the cursor. The list shows
    /// "Add New" at 0, then the purchases newest first.
    fn selected_purchase_entry(&self) -> Option<(usize, &Purchase)> {
        let purchases = self.selected_portfolio_position()?.get_purchases();
        purchases_newest_first(purchases)
            .get(self.selected_purchase.checked_sub(1)?)
            .copied()
    }

    /// The recorded `Price` of the selected position's purchase at `index`
    /// in the portfolio file.
    fn price_in_file(&self, index: usize) -> Option<f64> {
        let data: Vec<Value> = serde_json::from_str(&self.positions_str).ok()?;
        let position = self.selected_portfolio_position()?;
        data.iter()
            .filter_map(Value::as_object)
            .find(|obj| json_matches_position(obj, position))?
            .get("Purchases")?
            .as_array()?
            .get(index)?
            .get("Price")?
            .as_f64()
    }

    pub fn next_edit_field(&mut self) {
        self.edit_field = match self.edit_field {
            EditField::Date => EditField::Quantity,
            EditField::Quantity => EditField::Price,
            EditField::Price => EditField::Date,
        };
    }

    pub fn previous_edit_field(&mut self) {
        self.edit_field = match self.edit_field {
            EditField::Date => EditField::Price,
            EditField::Quantity => EditField::Date,
            EditField::Price => EditField::Quantity,
        };
    }

    pub fn get_current_input(&self) -> &str {
        match self.edit_field {
            EditField::Date => &self.purchase_date_input,
            EditField::Quantity => &self.purchase_quantity_input,
            EditField::Price => &self.purchase_price_input,
        }
    }

    pub fn get_current_input_mut(&mut self) -> &mut String {
        match self.edit_field {
            EditField::Date => &mut self.purchase_date_input,
            EditField::Quantity => &mut self.purchase_quantity_input,
            EditField::Price => &mut self.purchase_price_input,
        }
    }

    /// The `(quantity, price)` of the purchase form; price 0.0 when left
    /// empty (it is auto-filled later).
    fn validated_purchase_inputs(&self) -> Result<(f64, f64), ValidationError> {
        if self.purchase_date_input.trim().is_empty() {
            return Err(ValidationError::DateRequired);
        }
        if self.purchase_quantity_input.trim().is_empty() {
            return Err(ValidationError::QuantityRequired);
        }
        let quantity: f64 = self
            .purchase_quantity_input
            .parse()
            .map_err(|_| ValidationError::InvalidQuantity(self.purchase_quantity_input.clone()))?;
        if quantity <= 0.0 {
            return Err(ValidationError::NonPositiveQuantity(quantity));
        }
        let price: f64 = if self.purchase_price_input.trim().is_empty() {
            0.0
        } else {
            self.purchase_price_input
                .parse()
                .map_err(|_| ValidationError::InvalidPrice(self.purchase_price_input.clone()))?
        };
        if price < 0.0 {
            return Err(ValidationError::NegativePrice(price));
        }
        Ok((quantity, price))
    }

    /// Validate the form and write it with `save`. A write failure is shown
    /// as an error popup but still closes the form.
    fn save_purchase_form(
        &mut self,
        save: fn(&App, &str, f64, f64) -> Result<()>,
    ) -> Result<(), ValidationError> {
        let (quantity, price) = self.validated_purchase_inputs()?;
        if let Err(e) = save(self, &self.purchase_date_input, quantity, price) {
            // File I/O errors aren't validation errors; show them instead.
            self.error_message = Some(format!("Failed to save: {e:#}"));
        }
        self.exit_edit_mode();
        Ok(())
    }

    pub fn save_edited_purchase(&mut self) -> Result<(), ValidationError> {
        self.save_purchase_form(App::save_purchase_edit_to_file)
    }

    pub fn save_new_purchase(&mut self) -> Result<(), ValidationError> {
        self.save_purchase_form(App::save_purchase_to_file)
    }

    fn find_and_update_position_by_identifier<F>(
        &self,
        sorted_position_index: usize,
        updater: F,
    ) -> Result<()>
    where
        F: FnOnce(&mut Map<String, Value>) -> Result<()>,
    {
        // Decrypted .gpg portfolios must never be written back to disk.
        if self.data_file_path.ends_with(".gpg") {
            bail!(
                "refusing to overwrite encrypted portfolio file {}: \
                 writing decrypted data to disk is not supported",
                self.data_file_path
            );
        }

        let Some(portfolio) = &self.portfolio else {
            bail!("No portfolio available");
        };
        let Some(selected_position) = portfolio.positions.get(sorted_position_index) else {
            bail!("Invalid sorted position index: {sorted_position_index}");
        };

        // Parse the original file to preserve all data
        let mut original_data: Vec<Value> = serde_json::from_str(&self.positions_str)
            .wrap_err("failed to parse original portfolio data")?;

        // Find and update the position in original data by matching identifiers
        let Some(position_json) = original_data.iter_mut().find_map(|value| {
            value
                .as_object_mut()
                .filter(|obj| json_matches_position(obj, selected_position))
        }) else {
            bail!(
                "Could not find position '{}' in original data",
                selected_position.get_name()
            );
        };
        updater(position_json)?;

        // Give legacy positions stable ids on first write.
        ensure_json_position_ids(&mut original_data);

        let json_string = serde_json::to_string_pretty(&original_data)
            .wrap_err("failed to serialize portfolio data to JSON")?;
        write_atomic_with_backup(Path::new(&self.data_file_path), json_string.as_bytes())
            .wrap_err_with(|| format!("failed to write to file: {}", self.data_file_path))
    }

    pub(super) fn save_purchase_to_file(
        &self,
        date: &str,
        quantity: f64,
        price: f64,
    ) -> Result<()> {
        self.find_and_update_position_by_identifier(self.selected_position, |position_obj| {
            let purchases = position_obj
                .entry("Purchases".to_string())
                .or_insert_with(|| Value::Array(vec![]))
                .as_array_mut()
                .ok_or_else(|| eyre!("Purchases field is not an array"))?;

            let mut purchase = Map::new();
            purchase.insert("Date".to_string(), json!(date));
            purchase.insert("Quantity".to_string(), json_number(quantity));
            set_purchase_price(&mut purchase, price);
            purchases.push(Value::Object(purchase));

            update_amount_from_purchases(position_obj);
            Ok(())
        })
    }

    pub(super) fn save_purchase_edit_to_file(
        &self,
        date: &str,
        quantity: f64,
        price: f64,
    ) -> Result<()> {
        let original_index = self.selected_purchase_entry().map(|(index, _)| index);
        self.find_and_update_position_by_identifier(self.selected_position, |position_obj| {
            let purchases = position_obj
                .get_mut("Purchases")
                .ok_or_else(|| eyre!("No purchases found for this position"))?
                .as_array_mut()
                .ok_or_else(|| eyre!("Purchases field is not an array"))?;

            let Some(purchase) = original_index.and_then(|index| purchases.get_mut(index)) else {
                bail!("Could not find purchase to edit");
            };
            let purchase = purchase
                .as_object_mut()
                .ok_or_else(|| eyre!("Invalid purchase data - not an object"))?;
            purchase.insert("Date".to_string(), json!(date));
            purchase.insert("Quantity".to_string(), json_number(quantity));
            set_purchase_price(purchase, price);

            update_amount_from_purchases(position_obj);
            Ok(())
        })
    }
}

/// Purchases with their index in `purchases`, newest first as listed in the TUI.
pub(super) fn purchases_newest_first(purchases: &[Purchase]) -> Vec<(usize, &Purchase)> {
    let mut list: Vec<(usize, &Purchase)> = purchases.iter().enumerate().collect();
    list.sort_by(|a, b| {
        let date_a = a.1.date.as_deref().unwrap_or("");
        let date_b = b.1.date.as_deref().unwrap_or("");
        date_b.cmp(date_a)
    });
    list
}

/// `value` as a JSON number, 0.0 when it is not finite.
fn json_number(value: f64) -> Value {
    json!(if value.is_finite() { value } else { 0.0 })
}

/// Persist `Price` only when the user entered one (> 0), otherwise drop it.
fn set_purchase_price(purchase: &mut Map<String, Value>, price: f64) {
    if price > 0.0 {
        if let Some(num) = Number::from_f64(price) {
            purchase.insert("Price".to_string(), Value::Number(num));
        }
    } else {
        purchase.remove("Price");
    }
}

/// Set `Amount` to the total quantity of the position's purchases.
fn update_amount_from_purchases(position: &mut Map<String, Value>) {
    let total_quantity: f64 = position
        .get("Purchases")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|p| p.get("Quantity")?.as_f64())
        .sum();
    position.insert("Amount".to_string(), json_number(total_quantity));
}

/// Whether a raw JSON position object is the same position as `pos`.
///
/// Matches by `Id` when both sides have one, so two positions with the same
/// name, ticker and class are never confused. Falls back to name + ticker +
/// asset class for files written before ids existed.
pub(super) fn json_matches_position(obj: &Map<String, Value>, pos: &PortfolioPosition) -> bool {
    if let (Some(id), Some(json_id)) = (pos.id(), obj.get("Id").and_then(|v| v.as_str())) {
        return id == json_id;
    }
    let str_field = |key: &str| obj.get(key).and_then(|v| v.as_str());
    let name_match = match (pos.get_name_option(), obj.get("Name")) {
        (Some(name), Some(_)) => str_field("Name") == Some(name),
        (None, None) => true,
        _ => false,
    };
    let ticker_match = match (pos.get_ticker(), obj.get("Ticker")) {
        (Some(ticker), Some(_)) => str_field("Ticker") == Some(ticker),
        (None, None) => true,
        _ => false,
    };
    let asset_class_match = str_field("AssetClass") == Some(pos.get_asset_class());
    name_match && ticker_match && asset_class_match
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::position::{PortfolioPosition, PriceStatus};

    fn raw(id: Option<&str>, class: &str) -> Map<String, Value> {
        let mut v = serde_json::json!({"Ticker": "FAKE", "AssetClass": class, "Amount": 1});
        if let Some(id) = id {
            v["Id"] = serde_json::json!(id);
        }
        v.as_object().unwrap().clone()
    }

    #[test]
    fn test_json_matches_position_prefers_id() {
        let mut pos =
            PortfolioPosition::for_test(Some("FAKE"), "Stocks", 1.0, 10.0, PriceStatus::Live);
        // No ids: legacy ticker/class match.
        assert!(json_matches_position(&raw(None, "Stocks"), &pos));
        assert!(!json_matches_position(&raw(None, "Bonds"), &pos));
        // Same ticker and class but another id is a different position.
        pos.id = Some("pos_aaa".to_string());
        assert!(json_matches_position(&raw(Some("pos_aaa"), "Stocks"), &pos));
        assert!(!json_matches_position(
            &raw(Some("pos_bbb"), "Stocks"),
            &pos
        ));
        // Id on one side only: legacy match.
        assert!(json_matches_position(&raw(None, "Stocks"), &pos));
    }

    /// An app on a temporary portfolio file with one position and two
    /// purchases (the older one without a recorded price).
    fn purchase_app(dir: &tempfile::TempDir) -> (App, std::path::PathBuf) {
        let positions = r#"[{"Name": "Fake", "Ticker": "FAKE", "AssetClass": "Stocks", "Amount": 3,
            "Purchases": [{"Date": "2024-01-01", "Quantity": 1},
                          {"Date": "2025-01-01", "Quantity": 2, "Price": 10.5}]}]"#;
        let path = dir.path().join("positions.json");
        std::fs::write(&path, positions).unwrap();
        let mut app = App::new(
            "EUR".to_string(),
            positions.to_string(),
            path.to_string_lossy().into_owned(),
            DisabledComponents::default(),
        );
        let mut portfolio = Portfolio::new();
        for position in crate::position::from_string(positions).unwrap() {
            portfolio.add_position(position);
        }
        app.portfolio = Some(portfolio);
        (app, path)
    }

    fn saved_purchases(path: &Path) -> Value {
        let data: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        json!({"Amount": data[0]["Amount"], "Purchases": data[0]["Purchases"]})
    }

    #[test]
    fn test_add_purchase_updates_amount() {
        let dir = tempfile::tempdir().unwrap();
        let (app, path) = purchase_app(&dir);
        app.save_purchase_to_file("2026-01-01", 4.0, 0.0).unwrap();
        assert_eq!(
            saved_purchases(&path),
            json!({"Amount": 7.0, "Purchases": [
                {"Date": "2024-01-01", "Quantity": 1},
                {"Date": "2025-01-01", "Quantity": 2, "Price": 10.5},
                {"Date": "2026-01-01", "Quantity": 4.0}]})
        );
    }

    #[test]
    fn test_edit_purchase_maps_newest_first_to_file_order() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, path) = purchase_app(&dir);

        // Row 1 is the newest purchase, the second one in the file.
        app.selected_purchase = 1;
        app.enter_edit_purchase_mode();
        assert_eq!(app.mode, AppMode::EditPurchase);
        assert_eq!(app.purchase_date_input, "2025-01-01");
        assert_eq!(app.purchase_quantity_input, "2");
        assert_eq!(app.purchase_price_input, "10.5");

        // Row 2 has no recorded price, so the input stays empty.
        app.selected_purchase = 2;
        app.enter_edit_purchase_mode();
        assert_eq!(app.purchase_date_input, "2024-01-01");
        assert_eq!(app.purchase_price_input, "");

        app.save_purchase_edit_to_file("2023-06-01", 5.0, 9.0)
            .unwrap();
        assert_eq!(
            saved_purchases(&path),
            json!({"Amount": 7.0, "Purchases": [
                {"Date": "2023-06-01", "Quantity": 5.0, "Price": 9.0},
                {"Date": "2025-01-01", "Quantity": 2, "Price": 10.5}]})
        );
    }

    fn error_of(result: Result<()>) -> String {
        format!("{:#}", result.unwrap_err())
    }

    /// The app with the file content swapped for `positions_str`.
    fn with_file(dir: &tempfile::TempDir, positions_str: &str) -> App {
        let (mut app, _) = purchase_app(dir);
        app.positions_str = positions_str.to_string();
        app
    }

    #[test]
    fn test_new_app_starts_on_overview_in_normal_mode() {
        let app = App::new(
            "USD".into(),
            "[]".into(),
            "p.json".into(),
            DisabledComponents::default(),
        );
        assert_eq!(app.current_tab, Tab::Overview);
        assert_eq!(app.mode, AppMode::Normal);
        assert_eq!(app.network_status, NetworkStatus::Connected);
        assert!(app.portfolio.is_none() && !app.should_quit && !app.flash_state);
    }

    #[test]
    fn test_receives_background_updates() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _) = purchase_app(&dir);
        // No channels yet.
        assert!(!app.try_receive_portfolio_update());
        assert!(!app.try_receive_historic_update());

        let (portfolio_tx, portfolio_rx) = mpsc::unbounded_channel();
        app.set_portfolio_receiver(portfolio_rx);
        let (historic_tx, historic_rx) = mpsc::unbounded_channel();
        app.set_historic_receiver(historic_rx);
        // Channels without messages.
        assert!(!app.try_receive_portfolio_update());
        assert!(!app.try_receive_historic_update());

        portfolio_tx
            .send((Portfolio::new(), NetworkStatus::Partial))
            .unwrap();
        assert!(app.try_receive_portfolio_update());
        assert_eq!(app.network_status, NetworkStatus::Partial);
        assert!(app.portfolio.as_ref().unwrap().positions.is_empty());
        assert!(app.flash_state);

        historic_tx.send(vec![(0.0, 1.0)]).unwrap();
        assert!(app.try_receive_historic_update());
        assert_eq!(app.historic_data, Some(vec![(0.0, 1.0)]));
        assert!(!app.flash_state);
    }

    #[test]
    fn test_tabs_wrap_around() {
        let mut app = App::new(
            "USD".into(),
            "[]".into(),
            "p".into(),
            DisabledComponents::default(),
        );
        app.next_tab();
        assert_eq!(app.current_tab, Tab::Balances);
        app.next_tab();
        assert_eq!(app.current_tab, Tab::Overview);
        app.previous_tab();
        assert_eq!(app.current_tab, Tab::Balances);
        app.previous_tab();
        assert_eq!(app.current_tab, Tab::Overview);
    }

    fn priced(balance: f64) -> Portfolio {
        let mut portfolio = Portfolio::new();
        portfolio.add_position(PortfolioPosition::for_test(
            Some("FAKE"),
            "Stocks",
            1.0,
            balance,
            PriceStatus::Live,
        ));
        portfolio
    }

    #[test]
    fn test_trends_follow_value_changes() {
        let mut app = App::new(
            "USD".into(),
            "[]".into(),
            "p".into(),
            DisabledComponents::default(),
        );
        let trend = |app: &App| app.trends.get("FAKE").copied();
        app.update_trends(&priced(10.0));
        assert_eq!(trend(&app), Some(Trend::Neutral));
        app.update_trends(&priced(11.0));
        assert_eq!(trend(&app), Some(Trend::Up));
        // A sub-cent change keeps the trend.
        app.update_trends(&priced(11.005));
        assert_eq!(trend(&app), Some(Trend::Up));
        app.update_trends(&priced(5.0));
        assert_eq!(trend(&app), Some(Trend::Down));
        // A tiny change without a stored trend is neutral.
        app.trends.clear();
        app.update_trends(&priced(5.0));
        assert_eq!(trend(&app), Some(Trend::Neutral));
    }

    #[test]
    fn test_trend_colors_flash() {
        let mut app = App::new(
            "USD".into(),
            "[]".into(),
            "p".into(),
            DisabledComponents::default(),
        );
        app.trends.insert("up".into(), Trend::Up);
        app.trends.insert("down".into(), Trend::Down);
        app.trends.insert("flat".into(), Trend::Neutral);
        assert_eq!(app.get_trend_color("up", Color::White), Color::Green);
        assert_eq!(app.get_trend_color("down", Color::White), Color::Red);
        assert_eq!(app.get_trend_color("flat", Color::White), Color::White);
        assert_eq!(app.get_trend_color("unknown", Color::Cyan), Color::Cyan);
        app.flash_state = true;
        assert_eq!(app.get_trend_color("up", Color::White), Color::LightGreen);
        assert_eq!(app.get_trend_color("down", Color::White), Color::LightRed);
    }

    #[test]
    fn test_position_and_purchase_selection_stay_in_bounds() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _) = purchase_app(&dir);
        app.portfolio
            .as_mut()
            .unwrap()
            .add_position(PortfolioPosition::for_test(
                None,
                "Cash",
                5.0,
                5.0,
                PriceStatus::Live,
            ));
        app.select_previous();
        assert_eq!(app.selected_position, 0);
        app.select_next();
        app.select_next();
        assert_eq!(app.selected_position, 1);

        app.selected_position = 0;
        app.enter_edit_mode();
        assert_eq!(app.mode, AppMode::PurchaseList);
        for _ in 0..3 {
            app.select_next_purchase();
        }
        // "Add New" plus two purchases.
        assert_eq!(app.selected_purchase, 2);
        app.select_previous_purchase();
        app.select_previous_purchase();
        app.select_previous_purchase();
        assert_eq!(app.selected_purchase, 0);

        // Out of range (the file shrank): nothing to select or edit.
        app.mode = AppMode::Normal;
        app.selected_position = 9;
        app.select_next_purchase();
        assert_eq!(app.selected_purchase, 0);
        app.enter_edit_mode();
        assert_eq!(app.mode, AppMode::Normal);

        app.portfolio = None;
        app.selected_position = 0;
        app.select_next();
        assert_eq!(app.selected_position, 0);
    }

    #[test]
    fn test_purchase_form_modes_reset_inputs() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _) = purchase_app(&dir);
        app.purchase_date_input = "x".into();
        app.edit_field = EditField::Price;
        app.enter_add_purchase_mode();
        assert_eq!(app.mode, AppMode::AddPurchase);
        assert_eq!(app.edit_field, EditField::Date);
        assert_eq!(app.purchase_date_input, "");

        // Row 0 is "Add New": there is no purchase to edit.
        app.selected_purchase = 0;
        app.enter_edit_purchase_mode();
        assert_eq!(app.mode, AppMode::AddPurchase);

        // Without a readable file the price is not prefilled.
        app.positions_str = "not json".into();
        app.selected_purchase = 1;
        app.enter_edit_purchase_mode();
        assert_eq!(app.purchase_quantity_input, "2");
        assert_eq!(app.purchase_price_input, "");

        app.edit_input = "1".into();
        app.exit_edit_mode();
        assert_eq!(app.mode, AppMode::Normal);
        assert!(app.edit_input.is_empty() && app.purchase_quantity_input.is_empty());
    }

    #[test]
    fn test_edit_fields_cycle_both_ways() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _) = purchase_app(&dir);
        app.purchase_date_input = "d".into();
        app.purchase_quantity_input = "q".into();
        app.purchase_price_input = "p".into();
        let mut seen = vec![];
        for _ in 0..3 {
            seen.push(app.get_current_input().to_string());
            app.next_edit_field();
        }
        assert_eq!(seen, ["d", "q", "p"]);
        assert_eq!(app.edit_field, EditField::Date);
        app.previous_edit_field();
        assert_eq!(app.edit_field, EditField::Price);
        app.get_current_input_mut().push('!');
        app.previous_edit_field();
        assert_eq!(app.edit_field, EditField::Quantity);
        app.get_current_input_mut().push('!');
        app.previous_edit_field();
        assert_eq!(app.edit_field, EditField::Date);
        app.get_current_input_mut().push('!');
        assert_eq!(
            [
                &app.purchase_date_input,
                &app.purchase_quantity_input,
                &app.purchase_price_input
            ],
            ["d!", "q!", "p!"]
        );
    }

    #[test]
    fn test_purchase_form_validation() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, path) = purchase_app(&dir);
        let before = std::fs::read_to_string(&path).unwrap();
        let mut check = |date: &str, quantity: &str, price: &str| {
            app.purchase_date_input = date.into();
            app.purchase_quantity_input = quantity.into();
            app.purchase_price_input = price.into();
            app.save_new_purchase().unwrap_err().to_string()
        };
        assert_eq!(check(" ", "1", ""), "Date is required");
        assert_eq!(check("2025-01-01", "", ""), "Quantity is required");
        assert_eq!(check("2025-01-01", ".", ""), "Invalid quantity format: .");
        assert_eq!(
            check("2025-01-01", "0", ""),
            "Quantity must be positive, got 0"
        );
        assert_eq!(
            check("2025-01-01", "1", "1.2.3"),
            "Invalid price format: 1.2.3"
        );
        assert_eq!(
            check("2025-01-01", "1", "-2"),
            "Price cannot be negative, got -2"
        );
        // Nothing was written.
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    }

    #[test]
    fn test_saving_the_forms_writes_and_closes_them() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, path) = purchase_app(&dir);
        app.mode = AppMode::AddPurchase;
        app.purchase_date_input = "2026-02-03".into();
        app.purchase_quantity_input = "1.5".into();
        app.purchase_price_input = "12".into();
        app.save_new_purchase().unwrap();
        assert_eq!(app.mode, AppMode::Normal);
        assert!(app.error_message.is_none());
        let saved = saved_purchases(&path);
        assert_eq!(saved["Amount"], json!(4.5));
        assert_eq!(
            saved["Purchases"][2],
            json!({"Date": "2026-02-03", "Quantity": 1.5, "Price": 12.0})
        );

        // Edit the newest purchase (still the 2025 one in `positions_str`).
        app.selected_purchase = 1;
        app.mode = AppMode::EditPurchase;
        app.purchase_date_input = "2025-01-02".into();
        app.purchase_quantity_input = "3".into();
        app.purchase_price_input = String::new();
        app.save_edited_purchase().unwrap();
        assert_eq!(app.mode, AppMode::Normal);
        assert_eq!(
            saved_purchases(&path)["Purchases"][1],
            json!({"Date": "2025-01-02", "Quantity": 3.0})
        );
    }

    #[test]
    fn test_write_failures_are_shown_not_returned() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _) = purchase_app(&dir);
        app.data_file_path = dir.path().join("secret.json.gpg").to_string_lossy().into();
        app.mode = AppMode::AddPurchase;
        app.purchase_date_input = "2026-02-03".into();
        app.purchase_quantity_input = "1".into();
        app.save_new_purchase().unwrap();
        assert_eq!(app.mode, AppMode::Normal);
        let message = app.error_message.clone().unwrap();
        assert!(
            message.starts_with("Failed to save: refusing to overwrite encrypted"),
            "{message}"
        );
        assert!(!dir.path().join("secret.json.gpg").exists());
    }

    #[test]
    fn test_file_update_errors() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _) = purchase_app(&dir);

        app.data_file_path = dir
            .path()
            .join("missing/dir/p.json")
            .to_string_lossy()
            .into();
        let message = error_of(app.save_purchase_to_file("2026-01-01", 1.0, 0.0));
        assert!(
            message.starts_with("failed to write to file: "),
            "{message}"
        );

        app.positions_str = "{".into();
        assert!(error_of(app.save_purchase_to_file("2026-01-01", 1.0, 0.0))
            .starts_with("failed to parse original portfolio data"));

        app.positions_str = r#"[{"Name": "Other", "AssetClass": "Cash"}, 3]"#.into();
        assert_eq!(
            error_of(app.save_purchase_to_file("2026-01-01", 1.0, 0.0)),
            "Could not find position 'Fake' in original data"
        );

        app.selected_position = 5;
        assert_eq!(
            error_of(app.save_purchase_to_file("2026-01-01", 1.0, 0.0)),
            "Invalid sorted position index: 5"
        );

        app.portfolio = None;
        assert_eq!(
            error_of(app.save_purchase_to_file("2026-01-01", 1.0, 0.0)),
            "No portfolio available"
        );
    }

    const FAKE: &str = r#""Name": "Fake", "Ticker": "FAKE", "AssetClass": "Stocks""#;

    #[test]
    fn test_add_purchase_to_a_position_without_purchases() {
        let dir = tempfile::tempdir().unwrap();
        let app = with_file(&dir, &format!("[{{{FAKE}, \"Amount\": 3}}]"));
        app.save_purchase_to_file("2026-01-01", 2.0, f64::INFINITY)
            .unwrap();
        // A non-finite price is not saved; the amount is the purchase total.
        assert_eq!(
            saved_purchases(&dir.path().join("positions.json")),
            json!({"Amount": 2.0, "Purchases": [{"Date": "2026-01-01", "Quantity": 2.0}]})
        );

        let app = with_file(&dir, &format!("[{{{FAKE}, \"Purchases\": 5}}]"));
        assert_eq!(
            error_of(app.save_purchase_to_file("2026-01-01", 2.0, 0.0)),
            "Purchases field is not an array"
        );
    }

    #[test]
    fn test_edit_purchase_errors() {
        let dir = tempfile::tempdir().unwrap();
        let edit = |positions: &str, selected: usize| {
            let mut app = with_file(&dir, positions);
            app.selected_purchase = selected;
            error_of(app.save_purchase_edit_to_file("2026-01-01", 1.0, 1.0))
        };
        assert_eq!(
            edit(&format!("[{{{FAKE}}}]"), 1),
            "No purchases found for this position"
        );
        assert_eq!(
            edit(&format!("[{{{FAKE}, \"Purchases\": {{}}}}]"), 1),
            "Purchases field is not an array"
        );
        assert_eq!(
            edit(&format!("[{{{FAKE}, \"Purchases\": []}}]"), 1),
            "Could not find purchase to edit"
        );
        assert_eq!(
            edit(&format!("[{{{FAKE}, \"Purchases\": [1, 2]}}]"), 0),
            "Could not find purchase to edit"
        );
        assert_eq!(
            edit(&format!("[{{{FAKE}, \"Purchases\": [1, 2]}}]"), 1),
            "Invalid purchase data - not an object"
        );
    }

    #[test]
    fn test_json_helpers() {
        assert_eq!(json_number(f64::NAN), json!(0.0));
        assert_eq!(json_number(1.5), json!(1.5));

        let mut purchase = Map::new();
        set_purchase_price(&mut purchase, 2.5);
        assert_eq!(purchase["Price"], json!(2.5));
        // Not finite: the old price stays.
        set_purchase_price(&mut purchase, f64::INFINITY);
        assert_eq!(purchase["Price"], json!(2.5));
        set_purchase_price(&mut purchase, 0.0);
        assert!(!purchase.contains_key("Price"));

        let mut position = Map::new();
        update_amount_from_purchases(&mut position);
        assert_eq!(position["Amount"], json!(0.0));
    }

    #[test]
    fn test_json_matches_position_by_name_and_ticker() {
        let positions = crate::position::from_string(
            r#"[{"Name": "N", "Ticker": "T", "AssetClass": "Stocks", "Amount": 1},
                {"AssetClass": "Cash", "Amount": 1}]"#,
        )
        .unwrap();
        let (named, bare) = (&positions[0], &positions[1]);
        let obj = |v: Value| v.as_object().unwrap().clone();
        assert!(json_matches_position(
            &obj(json!({"Name": "N", "Ticker": "T", "AssetClass": "Stocks"})),
            named
        ));
        assert!(!json_matches_position(
            &obj(json!({"Name": "X", "Ticker": "T", "AssetClass": "Stocks"})),
            named
        ));
        assert!(!json_matches_position(
            &obj(json!({"Name": "N", "Ticker": "X", "AssetClass": "Stocks"})),
            named
        ));
        assert!(!json_matches_position(
            &obj(json!({"Ticker": "T", "AssetClass": "Stocks"})),
            named
        ));
        assert!(json_matches_position(
            &obj(json!({"AssetClass": "Cash"})),
            bare
        ));
        assert!(!json_matches_position(
            &obj(json!({"Name": "N", "AssetClass": "Cash"})),
            bare
        ));
        assert!(!json_matches_position(
            &obj(json!({"Ticker": "T", "AssetClass": "Cash"})),
            bare
        ));
    }

    #[test]
    fn test_purchases_newest_first_puts_undated_last() {
        let purchases: Vec<Purchase> = serde_json::from_str(
            r#"[{"Quantity": 1}, {"Date": "2024-01-01", "Quantity": 2},
                {"Date": "2025-01-01", "Quantity": 3}]"#,
        )
        .unwrap();
        let order: Vec<usize> = purchases_newest_first(&purchases)
            .into_iter()
            .map(|(index, _)| index)
            .collect();
        assert_eq!(order, [2, 1, 0]);
    }
}
