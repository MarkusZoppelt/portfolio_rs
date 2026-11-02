//! Key handling: pure state transitions on [`App`].

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};

use super::{App, AppMode, EditField, Tab};

/// What the event loop must do after a key was handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Action {
    /// Nothing beyond the state change already made.
    None,
    /// Re-read the portfolio file and revalue it with live quotes.
    Reload,
}

/// Apply `key` to `app`. Only key presses count; repeats and releases are ignored.
pub(super) fn handle_key(app: &mut App, key: KeyEvent) -> Action {
    if key.kind != KeyEventKind::Press {
        return Action::None;
    }
    match app.mode {
        AppMode::Normal => return handle_normal_key(app, key.code),
        AppMode::PurchaseList => handle_purchase_list_key(app, key.code),
        AppMode::AddPurchase | AppMode::EditPurchase => return handle_form_key(app, key.code),
        // Legacy amount editor: only cancelling is supported.
        AppMode::Edit if key.code == KeyCode::Esc => app.exit_edit_mode(),
        AppMode::Edit => {}
    }
    Action::None
}

fn handle_normal_key(app: &mut App, code: KeyCode) -> Action {
    let on_balances = app.current_tab == Tab::Balances;
    match code {
        KeyCode::Char('q') | KeyCode::Esc => app.should_quit = true,
        // Vim navigation - hjkl
        KeyCode::Char('h') | KeyCode::Left | KeyCode::BackTab => app.previous_tab(),
        KeyCode::Char('l') | KeyCode::Right | KeyCode::Tab => app.next_tab(),
        KeyCode::Char('j') | KeyCode::Down if on_balances => app.select_next(),
        KeyCode::Char('k') | KeyCode::Up if on_balances => app.select_previous(),
        KeyCode::Char('e') if on_balances => app.enter_edit_mode(),
        // Manual refresh: read latest file and rebuild portfolio immediately
        KeyCode::Char('r') => return Action::Reload,
        KeyCode::Char('1') => app.current_tab = Tab::Overview,
        KeyCode::Char('2') => app.current_tab = Tab::Balances,
        _ => {}
    }
    Action::None
}

fn handle_purchase_list_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Esc => app.exit_edit_mode(),
        KeyCode::Char('j') | KeyCode::Down => app.select_next_purchase(),
        KeyCode::Char('k') | KeyCode::Up => app.select_previous_purchase(),
        // Row 0 adds a new purchase, the others edit an existing one.
        KeyCode::Char('a') | KeyCode::Enter if app.selected_purchase == 0 => {
            app.enter_add_purchase_mode();
        }
        KeyCode::Char('a') | KeyCode::Enter => app.enter_edit_purchase_mode(),
        _ => {}
    }
}

/// Keys of the add and edit purchase forms.
fn handle_form_key(app: &mut App, code: KeyCode) -> Action {
    match code {
        KeyCode::Esc => {
            app.mode = AppMode::PurchaseList;
            app.clear_purchase_inputs();
        }
        KeyCode::Tab | KeyCode::Down => app.next_edit_field(),
        KeyCode::Up => app.previous_edit_field(),
        KeyCode::Enter => {
            let saved = if app.mode == AppMode::AddPurchase {
                app.save_new_purchase()
            } else {
                app.save_edited_purchase()
            };
            match saved {
                // Pick up the new file content and refresh immediately
                Ok(()) => return Action::Reload,
                Err(e) => app.error_message = Some(e.to_string()),
            }
        }
        KeyCode::Backspace => {
            app.get_current_input_mut().pop();
        }
        KeyCode::Char(c) => type_form_char(app, c),
        _ => {}
    }
    Action::None
}

/// Type `c` into the focused form field. Dates take digits only and get
/// their dashes automatically (`YYYY-MM-DD`); quantity and price take digits
/// and one decimal point.
fn type_form_char(app: &mut App, c: char) {
    match app.edit_field {
        EditField::Date if c.is_ascii_digit() => {
            let current = app.get_current_input_mut();
            // Count digits only (ignoring dashes)
            if current.chars().filter(char::is_ascii_digit).count() < 8 {
                current.push(c);
                // Auto-add dashes at positions 4 and 7 (after YYYY and MM)
                if current.len() == 4 || current.len() == 7 {
                    current.push('-');
                }
            }
        }
        EditField::Date => {}
        EditField::Quantity | EditField::Price => {
            if c.is_ascii_digit() || (c == '.' && !app.get_current_input().contains('.')) {
                app.get_current_input_mut().push(c);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::portfolio::Portfolio;
    use crate::tui::DisabledComponents;
    use crossterm::event::{KeyEventState, KeyModifiers};

    const POSITIONS: &str = r#"[
      {"Name": "A", "Ticker": "AAA", "AssetClass": "Stocks", "Amount": 1,
       "Purchases": [{"Date": "2024-01-01", "Quantity": 1, "Price": 5}]},
      {"Name": "B", "AssetClass": "Cash", "Amount": 2}]"#;

    fn app(dir: &tempfile::TempDir) -> App {
        let path = dir.path().join("positions.json");
        std::fs::write(&path, POSITIONS).unwrap();
        let mut app = App::new(
            "USD".into(),
            POSITIONS.into(),
            path.to_string_lossy().into(),
            DisabledComponents::default(),
        );
        let mut portfolio = Portfolio::new();
        for position in crate::position::from_string(POSITIONS).unwrap() {
            portfolio.add_position(position);
        }
        app.set_portfolio(portfolio);
        app
    }

    fn press(app: &mut App, code: KeyCode) -> Action {
        handle_key(app, KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn type_str(app: &mut App, text: &str) {
        for c in text.chars() {
            assert_eq!(press(app, KeyCode::Char(c)), Action::None);
        }
    }

    #[test]
    fn only_key_presses_count() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app(&dir);
        let release = KeyEvent {
            code: KeyCode::Char('q'),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Release,
            state: KeyEventState::NONE,
        };
        assert_eq!(handle_key(&mut app, release), Action::None);
        assert!(!app.should_quit);
    }

    #[test]
    fn normal_mode_switches_tabs_and_quits() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app(&dir);
        for (code, tab) in [
            (KeyCode::Char('l'), Tab::Balances),
            (KeyCode::Right, Tab::Overview),
            (KeyCode::Tab, Tab::Balances),
            (KeyCode::Char('h'), Tab::Overview),
            (KeyCode::Left, Tab::Balances),
            (KeyCode::BackTab, Tab::Overview),
            (KeyCode::Char('2'), Tab::Balances),
            (KeyCode::Char('1'), Tab::Overview),
        ] {
            assert_eq!(press(&mut app, code), Action::None);
            assert_eq!(app.current_tab, tab, "{code:?}");
        }
        assert_eq!(press(&mut app, KeyCode::Char('r')), Action::Reload);
        assert_eq!(press(&mut app, KeyCode::Char('x')), Action::None);
        assert!(!app.should_quit);
        press(&mut app, KeyCode::Char('q'));
        assert!(app.should_quit);
        app.should_quit = false;
        press(&mut app, KeyCode::Esc);
        assert!(app.should_quit);
    }

    #[test]
    fn selection_and_editing_only_work_on_balances() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app(&dir);
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Char('e'));
        assert_eq!((app.selected_position, app.mode), (0, AppMode::Normal));

        app.current_tab = Tab::Balances;
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(app.selected_position, 1);
        press(&mut app, KeyCode::Up);
        assert_eq!(app.selected_position, 0);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char('k'));
        assert_eq!(app.selected_position, 0);
        press(&mut app, KeyCode::Char('e'));
        assert_eq!(app.mode, AppMode::PurchaseList);
    }

    #[test]
    fn purchase_list_navigates_adds_and_edits() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app(&dir);
        app.mode = AppMode::PurchaseList;
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Down);
        assert_eq!(app.selected_purchase, 1);
        press(&mut app, KeyCode::Char('k'));
        assert_eq!(app.selected_purchase, 0);
        press(&mut app, KeyCode::Char('z'));
        assert_eq!(app.mode, AppMode::PurchaseList);

        press(&mut app, KeyCode::Char('a'));
        assert_eq!(app.mode, AppMode::AddPurchase);

        app.mode = AppMode::PurchaseList;
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.mode, AppMode::EditPurchase);
        assert_eq!(app.purchase_date_input, "2024-01-01");
        assert_eq!(app.purchase_price_input, "5");

        app.mode = AppMode::PurchaseList;
        press(&mut app, KeyCode::Up);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.mode, AppMode::AddPurchase);

        app.mode = AppMode::PurchaseList;
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.mode, AppMode::Normal);
    }

    #[test]
    fn date_field_takes_digits_and_adds_dashes() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app(&dir);
        app.enter_add_purchase_mode();
        type_str(&mut app, "2025x0a3150");
        // Digits only, dashes added, at most eight digits.
        assert_eq!(app.purchase_date_input, "2025-03-15");
        press(&mut app, KeyCode::Backspace);
        assert_eq!(app.purchase_date_input, "2025-03-1");
    }

    #[test]
    fn number_fields_take_one_decimal_point() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app(&dir);
        app.enter_add_purchase_mode();
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.edit_field, EditField::Quantity);
        type_str(&mut app, "1.5.x2");
        assert_eq!(app.purchase_quantity_input, "1.52");
        press(&mut app, KeyCode::Down);
        assert_eq!(app.edit_field, EditField::Price);
        type_str(&mut app, ".7");
        assert_eq!(app.purchase_price_input, ".7");
        press(&mut app, KeyCode::Up);
        assert_eq!(app.edit_field, EditField::Quantity);
        assert_eq!(press(&mut app, KeyCode::F(1)), Action::None);
    }

    #[test]
    fn form_escape_returns_to_the_list() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app(&dir);
        for mode in [AppMode::AddPurchase, AppMode::EditPurchase] {
            app.mode = mode;
            app.purchase_quantity_input = "3".into();
            press(&mut app, KeyCode::Esc);
            assert_eq!(app.mode, AppMode::PurchaseList);
            assert!(app.purchase_quantity_input.is_empty());
        }
    }

    #[test]
    fn saving_a_valid_form_asks_for_a_reload() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app(&dir);
        app.enter_add_purchase_mode();
        type_str(&mut app, "20250102");
        press(&mut app, KeyCode::Tab);
        type_str(&mut app, "2");
        assert_eq!(press(&mut app, KeyCode::Enter), Action::Reload);
        assert_eq!(app.mode, AppMode::Normal);
        let saved = std::fs::read_to_string(&app.data_file_path).unwrap();
        assert!(saved.contains("2025-01-02"), "{saved}");

        // Edit the only purchase of the file on disk.
        app.selected_purchase = 1;
        app.enter_edit_purchase_mode();
        press(&mut app, KeyCode::Tab);
        type_str(&mut app, "0");
        assert_eq!(press(&mut app, KeyCode::Enter), Action::Reload);
        let saved = std::fs::read_to_string(&app.data_file_path).unwrap();
        assert!(saved.contains("\"Quantity\": 10.0"), "{saved}");
    }

    #[test]
    fn an_invalid_form_shows_the_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app(&dir);
        app.enter_add_purchase_mode();
        assert_eq!(press(&mut app, KeyCode::Enter), Action::None);
        assert_eq!(app.error_message.as_deref(), Some("Date is required"));
        assert_eq!(app.mode, AppMode::AddPurchase);

        app.mode = AppMode::EditPurchase;
        app.purchase_date_input = "2025-01-01".into();
        assert_eq!(press(&mut app, KeyCode::Enter), Action::None);
        assert_eq!(app.error_message.as_deref(), Some("Quantity is required"));
    }

    #[test]
    fn legacy_edit_mode_only_cancels() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app(&dir);
        app.mode = AppMode::Edit;
        app.edit_input = "12".into();
        press(&mut app, KeyCode::Char('q'));
        assert_eq!(app.mode, AppMode::Edit);
        assert!(!app.should_quit);
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.mode, AppMode::Normal);
        assert!(app.edit_input.is_empty());
    }
}
