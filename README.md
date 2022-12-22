<p align="center">
    <img src="https://raw.githubusercontent.com/MarkusZoppelt/portfolio_rs/main/img/logo.png" alt="portfolio_rs logo"><br>
    <img src="https://github.com/markuszoppelt/portfolio_rs/actions/workflows/rust.yml/badge.svg" alt="build status badge">
	<img src="https://github.com/MarkusZoppelt/portfolio_rs/actions/workflows/rust-clippy.yml/badge.svg" alt="clippy analyze status badge">
</p>

A command line tool for managing financial investment portfolios written in Rust.

> *This project is the modern successor of [finance](https://github.com/MarkusZoppelt/finance).*

## Installation
You can install portfolio\_rs directly from cargo (via crates.io:

    cargo install portfolio_rs

## Usage 

### 1. Create your portfolio file
Create a JSON file with your portfolio positions.

Look at the [example data](example_data.json) for the format and data scheme.

### 2. Use the subcommands to gain insight on your portfolio:
Show the current balances of your portfolio: 

    portfolio_rs balances <JSON_FILE>

Show the current allocation of your portfolio: 

    portfolio_rs allocation <JSON_FILE>

Show the performance of your portfolio:
    
    portfolio_rs performance <JSON_FILE>


If you need help, try `portfolio_rs help [SUBCOMMAND]` for usage information.


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
