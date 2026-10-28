# Quotes, currencies and returns

## Quote sources

Quotes come from Yahoo Finance by default. Set `PORTFOLIO_RS_QUOTES` to choose
another source: `offline` makes no network calls (nothing gets a price), and
`fixture:<path>` serves prices from a JSON file with the shape of
[`tests/fixtures/quotes_example.json`](../tests/fixtures/quotes_example.json). An
invalid value prints a warning and uses offline quotes; it never falls back to
the network.

    PORTFOLIO_RS_QUOTES=fixture:tests/fixtures/quotes_example.json portfolio_rs context example_data.json

The MCP server and HTTP API also keep a last-known-price cache next to the
positions file; see [MCP server](mcp.md#last-known-price-cache).

## Base currency

`currency` in the [config file](configuration.md) (default `EUR`) is the base
currency for totals. Quotes in another currency (as reported by Yahoo) are
converted with one FX quote per currency, e.g. `EURUSD=X`; only the ticker
symbol is sent. Prices and average cost are shown in the quote currency (e.g.
`512.30 USD`); values, invested amounts, and totals are in the base currency.

Purchase prices are treated as quote-currency prices and converted at the
current FX rate, so cost basis ignores past FX moves (MCP responses flag this
as `cost_basis_current_fx`). If an FX quote, or the quote currency itself, is
unknown, the affected positions are flagged, their invested amount is null,
and they are excluded from totals rather than mixed in unconverted.

## `performance` returns

`performance` period returns (1W, 1M, 3M, 6M, YTD, 1Y) are the market return
of the ticker positions you hold now, at current quantities, compared like for
like: cash, ticker-less holdings, unpriced positions, and positions without a
price at the period start are left out of both sides. Deposits, withdrawals,
and trades during the period are not accounted for. 1D and "Since" compare
total portfolio values (including cash).

For contribution-aware figures from stored snapshots, see `get_performance` in
the [MCP server](mcp.md#performance-and-snapshots) docs.
