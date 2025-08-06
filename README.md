<p align="center">
    <img src="https://raw.githubusercontent.com/MarkusZoppelt/portfolio_rs/main/img/logo.png" alt="portfolio_rs logo"><br>
    <img src="https://github.com/markuszoppelt/portfolio_rs/actions/workflows/rust.yml/badge.svg" alt="build status badge">
</p>

A command line tool with interactive TUI for managing financial investment portfolios written in Rust.

> *This project is the modern successor of [finance](https://github.com/MarkusZoppelt/finance).*

## Installation

Install/run via [pkgx](https://pkgx.sh):

    pkgx portfolio_rs

You can install portfolio\_rs directly from cargo (via crates.io):

    cargo install portfolio_rs

## Usage 

### 1. Create your portfolio file
Create a JSON file with your portfolio positions.

Look at the [example data](example_data.json) for the format and data scheme.

### 2. Launch the portfolio tool:

**Default: Customizable Interactive TUI** (recommended):

    portfolio_rs [JSON_FILE] [--tab TAB] [--disable COMPONENTS]

You may *optionally* specify which tab to open at start-up:

    portfolio_rs [JSON_FILE] --tab overview     # Start on Overview & Allocation tab (default)
    portfolio_rs [JSON_FILE] --tab balances     # Start on Balances tab

**CLI Commands** (optional):

    portfolio_rs balances [JSON_FILE]     # Show balances table
    portfolio_rs allocation [JSON_FILE]   # Show allocation chart  
    portfolio_rs performance [JSON_FILE]  # Show performance metrics

**Configuration:**

    portfolio_rs config                   # Show config file location
    portfolio_rs components               # Show available components

If no file is specified, the tool uses the file from your config. If you need help, try `portfolio_rs --help` for usage information.

## TUI Features

The interactive Terminal User Interface (default mode) provides:

- **Overview & Allocation Tab**: Large display of total portfolio value, visual bar chart, and detailed allocation breakdown
- **Balances Tab**: Detailed table of all positions with amounts, current values, and **edit functionality**

### TUI Customization

You can disable specific UI components using the `--disable` flag with comma-separated component names:

**Overview Tab Components:**
- `tab_bar` - Top navigation tabs
- `total_value` - Large portfolio value display
- `asset_allocation` - Visual bar chart
- `detailed_allocation` - Allocation percentage list
- `help` - Help text at bottom

**Balances Tab Components:**
- `tab_bar` - Top navigation tabs
- `name` - Position name column
- `asset_class` - Asset class column  
- `amount` - Amount/quantity column
- `balance` - Balance/value column

**Examples:**
```bash
# Hide tab bar and help text
portfolio_rs --disable tab_bar,help example_data.json

# Show only the allocation chart (hide detailed list)
portfolio_rs --disable detailed_allocation example_data.json

# Minimal balances view (name and balance only)
portfolio_rs --disable asset_class,amount example_data.json
```

### TUI Navigation
- `h` / `l` : Switch tabs left/right (vim-style)
- `j` / `k` : Navigate up/down (select positions in Balances tab)
- `e` : Edit selected position amount (in Balances tab)
- `Tab` / `←` `→` : Switch between tabs
- `1-2` : Jump directly to specific tabs
- `q` / `Esc` : Quit the application

### Edit Functionality
- Select any position with `j`/`k` and press `e` to edit
- Real-time balance preview and input validation
- Changes are saved automatically to your data file
- Supports decimal precision for crypto and fractional shares


## Screenshots

### Overview & Allocation Tab
![TUI Overview](https://raw.githubusercontent.com/MarkusZoppelt/portfolio_rs/main/img/tui_overview.png)

### Balances Tab with Edit Functionality
![TUI Balances](https://raw.githubusercontent.com/MarkusZoppelt/portfolio_rs/main/img/tui_balances.png)

## Demo
![demo](https://raw.githubusercontent.com/MarkusZoppelt/portfolio_rs/main/img/demo.gif)

## Configuration
Upon first run, `portfolio_rs` will create a default config file.
Use `portfolio_rs config` to show the config file location.

The most useful config entry is `portfolio_file` where you can set the **absolute** path to your data file. This will be used when no file is specified as an argument.

## Bonus: GPG Encryption
This tool supports (gpg) encrypted json files.
Decrypted values are never written to disk.

    # you will need a valid gpg key in ~/.gnupg/
    portfolio_rs [COMMAND] data.json.gpg

Pro Tip: Use a plugin like [vim-gnupg](https://github.com/jamessan/vim-gnupg)
for editing your data file.
