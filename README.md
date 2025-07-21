<p align="center">
    <img src="https://raw.githubusercontent.com/MarkusZoppelt/portfolio_rs/main/img/logo.png" alt="portfolio_rs logo"><br>
    <img src="https://github.com/markuszoppelt/portfolio_rs/actions/workflows/rust.yml/badge.svg" alt="build status badge">
	<img src="https://github.com/MarkusZoppelt/portfolio_rs/actions/workflows/rust-clippy.yml/badge.svg" alt="clippy analyze status badge">
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

### 2. Use the subcommands to gain insight on your portfolio:

Launch the interactive TUI (Terminal User Interface):

    portfolio_rs tui <JSON_FILE>

Show the current balances of your portfolio: 

    portfolio_rs balances <JSON_FILE>

Show the current allocation of your portfolio: 

    portfolio_rs allocation <JSON_FILE>

Show the performance of your portfolio:
    
    portfolio_rs performance <JSON_FILE>


If you need help, try `portfolio_rs help [SUBCOMMAND]` for usage information.

## TUI Features

The interactive Terminal User Interface (`portfolio_rs tui`) provides:

- **Overview & Allocation Tab**: Large display of total portfolio value, visual bar chart, and detailed allocation breakdown
- **Balances Tab**: Detailed table of all positions with amounts and current values  
- **Performance Tab**: Performance metrics (YTD, monthly, recent changes)

### TUI Navigation
- `h` / `l` : Switch tabs left/right (vim-style)
- `j` / `k` : Navigate up/down
- `Tab` / `←` `→` : Switch between tabs
- `1-3` : Jump directly to specific tabs
- `q` / `Esc` : Quit the application


## Demo
![demo](https://raw.githubusercontent.com/MarkusZoppelt/portfolio_rs/main/img/demo.gif)

## Configuration
Upon first run, `portfolio_rs` will create a default `config.yml` file.
The location of the config file depends on the operating system.
Use `portfolio_rs config` to print the config directory.

Probably the most useful entry in the config is `portfolio_file` where you can
set the **absolute** path to a data file that will be used when no data file is
passed as an argument.

## Bonus: GPG Encryption
This tool supports (gpg) encrypted json files.
Decrypted values are never written to disk.

    # you will need a valid gpg key in ~/.gnupg/
    portfolio_rs [COMMAND] data.json.gpg

Pro Tip: Use a plugin like [vim-gnupg](https://github.com/jamessan/vim-gnupg)
for editing your data file.
