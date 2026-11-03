# Vendored crate

This crate is vendored from the [tom](https://github.com/solos/tom)
workspace so that Terry can embed the tom agent runtime without publishing the
crates or tracking an external version.

tom is a Rust port of pi, and is MIT licensed. The upstream `LICENSE` is
reproduced alongside this file. Terry ships it as GPL-3.0-or-later, which MIT
permits; keep the copyright notice when redistributing.

Synced from tom at tag `v0.17.0` (commit `1c078e8`, 2026-11-03).

Upstream tagged `v0.17.0` at `1c078e8` without bumping the workspace version
(tagging happens before release-plz bumps and publishes), so its `Cargo.toml`
still reads `0.16.0`. The crates here carry `0.17.0` to match the tag they were
copied from; without that, `tom --version` and plugin `host.version` would
report one release behind the code they run.

Upstream crate: `crates/` + this directory name. Local adaptations:

- Kept on Rust edition 2021 (Terry's workspace is edition 2024). The code
  predates edition 2024's `unsafe` requirement on `std::env::set_var`.

Do not edit the sources here for Terry-specific behaviour. Put integration code
in the host application instead, so re-vendoring stays a copy.

## Local modifications

`tom-cli` is the one vendored crate with source changes, because it is the
entry point the host drives:

- `app::run_with_args(argv)` — added so the host can pass an explicit argument
  list. `app::run()` still reads `std::env::args()` for the standalone binary;
  the host must pass arguments *after* its subcommand name, or that name becomes
  the initial prompt.
- `host_state` module — reports the agent's run state to the host terminal over
  its local IPC API. Disabled when the host's `TERRY_*` environment variables are
  absent.
- `app::terry_agent_theme()` — reads `TERRY_AGENT_THEME` when neither `--theme`
  nor the saved `theme` setting is present, so the agent follows the hosting
  terminal's palette.

Everything else matches upstream. Re-vendoring means replaying these three
changes; they are all additive and none alter the CLI's behaviour when the
`TERRY_*` variables are unset.
