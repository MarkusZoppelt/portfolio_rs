# TUI

The interactive Terminal User Interface (default mode) provides:

- **Overview & Allocation Tab**: Large display of total portfolio value, visual bar chart, and detailed allocation breakdown
- **Balances Tab**: Detailed table of all positions with amounts, current values, and **edit functionality**

## Customization

You may optionally specify which tab to open at start-up:

    portfolio_rs [JSON_FILE] --tab overview     # Start on Overview & Allocation tab (default)
    portfolio_rs [JSON_FILE] --tab balances     # Start on Balances tab

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

## Navigation

- `h` / `l` : Switch tabs left/right (vim-style)
- `j` / `k` : Navigate up/down (select positions in Balances tab)
- `e` : Edit selected position amount (in Balances tab)
- `Tab` / `←` `→` : Switch between tabs
- `1-2` : Jump directly to specific tabs
- `q` / `Esc` : Quit the application

## Editing

- Select any position with `j`/`k` and press `e` to edit
- Real-time balance preview and input validation
- Changes are saved automatically to your data file
- Supports decimal precision for crypto and fractional shares
