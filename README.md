<p align="center">
    <img src="img/logo.png" alt="portfolio_rs logo"><br>
    <img src="https://github.com/markuszoppelt/portfolio_rs/actions/workflows/rust.yml/badge.svg" alt="build status badge">
	<img src="https://github.com/MarkusZoppelt/portfolio_rs/actions/workflows/rust-clippy.yml/badge.svg" alt="clippy analyze status badge">
</p>

A command line tool for managing financial investment portfolios written in Rust.

*This project is meant to be the modern successor of my [finance](https://github.com/MarkusZoppelt/finance) repository.*

## Installation

You can install portfolio\_rs directly from cargo (via crates.io:

    cargo install portfolio_rs

## Usage 

You can try subcommands, e.g., `balances` or `allocation` with the example data.
If you need help, try `portfolio_rs help` for usage information.

## Roadmap

- [x] Async quote querying
- [x] JSON format for portfolio position
- [x] Plot portfolio allocation
- [ ] Store quote data persistently. (I want to try [SurrealDB](https://github.com/surrealdb/surrealdb))
- [ ] Performance tracking

## Examples:

    portfolio_rs balances example_data.json

will result in something like:

                          Name |  Asset Class |     Amount |    Balance
    ====================================================================
                       S&P 500 |       Stocks |       2.00 |     749.28
             US Treasury 20+yr |        Bonds |       4.00 |     420.64
                   Commodities |  Commodities |       3.00 |      64.49
                          Gold |         Gold |       1.00 |     155.68
                       Bitcoin |       Crypto |       0.01 |     189.63
                          Cash |         Cash |     200.00 |     200.00
    ====================================================================
    Your total balance is: 1779.72

