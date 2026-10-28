# Configuration

On first run, `portfolio_rs` creates a default config file.
`portfolio_rs config` shows its location.

- `portfolio_file`: the **absolute** path to your data file, used when no file
  is given as an argument.
- `workspace_dir`: your workspace, for workspace users. The workspace's
  `positions.json` then takes precedence.
- `currency`: the base currency for totals (default `EUR`). See
  [Quotes, currencies and returns](quotes.md#base-currency).
- `llm_api_key`: a write-only secret. The `AppConfigDto` JSON view used by GUIs
  always serializes it as `null` and exposes `hasLlmApiKey` instead. To clear a
  stored key, set it to an empty string. A null or omitted key on an update
  does not wipe it.

## GPG encryption

Portfolio files can be gpg-encrypted JSON. Decrypted values are never written
to disk.

    # you will need a valid gpg key in ~/.gnupg/
    portfolio_rs [COMMAND] data.json.gpg

Pro tip: use a plugin like [vim-gnupg](https://github.com/jamessan/vim-gnupg)
to edit your data file.
