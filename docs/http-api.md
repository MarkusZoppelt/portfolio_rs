# HTTP API

Serve your portfolio to local scripts, agents, and GUIs:

    portfolio_rs api positions.json [--policy portfolio/policy.toml] [--host 127.0.0.1] [--port 3000] [--token-file PATH]

| Endpoint | Description |
|---|---|
| `GET /health` | Liveness check |
| `GET /api/portfolio` | Full portfolio summary, with `complete`, `hasStale` and `dataStatus` |
| `GET/POST /api/positions` | List / create positions |
| `GET/PUT/DELETE /api/positions/{id}` | Read / update / delete a position by stable `pos_` id (`positionId`; a numeric index still works) |
| `GET /api/allocation` | Allocation breakdown |
| `GET /api/performance` | Performance metrics |
| `GET /api/context` | Agent briefing (JSON) |
| `GET /api/review` | Policy review (requires `--policy`) |
| `GET /api/simulate` | Rebalance simulation (requires `--policy`) |
| `GET /api/validate` | Portfolio file validation |
| `GET /api/doctor?dir=DIR` | Workspace health check |
| `POST /api/refresh` | Force a live quote refresh |

Notes:

- The API is intended for **local use only**; binding to a non-loopback host
  prints a loud warning. Requests must use `Host: localhost:<port>`,
  `127.0.0.1:<port>` or `[::1]:<port>` (anything else gets `421`), which
  blocks DNS-rebinding attacks from web pages.
- `--token-file PATH` turns on bearer auth: a random token is written to
  `PATH` (mode 0600, never printed or logged) and every `/api/*` request
  needs `Authorization: Bearer $(cat PATH)` (`401` otherwise). `/health`
  stays open.
- Positions carry a stable `positionId` (`pos_…`, generated if missing and
  persisted on the next save; a stored `Id` always wins). Generated ids of
  market positions derive from ticker and asset class only, so renaming a
  position keeps its id; cash and manual positions use name and asset class.
  Use it in `/api/positions/{id}`. The numeric
  `id` is the array index, kept for older clients.
- Purchase `price` is the recorded price only; prices estimated from
  historic closes are `null`.
- Position mutations are persisted back to the portfolio file, except for
  `.gpg` files, which are never rewritten.
- Live quotes are cached for a short TTL; `POST /api/refresh` bypasses the
  cache.
- Application config is not an API payload. Embedding clients that read
  `AppConfigDto` still never receive the LLM API key: `llmApiKey` is always
  `null`, and `hasLlmApiKey` reports whether a non-empty key is configured.
  A null or missing `llmApiKey` does not clear a stored key; an empty string
  passed to `AppConfig::set_llm_api_key` (or `AppConfigDto::apply_to`) does.

