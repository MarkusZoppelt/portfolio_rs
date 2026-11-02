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
