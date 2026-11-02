# Vendored crate

This crate is vendored from the [tom](https://github.com/solos/tom)
workspace so that Terry can embed the tom agent runtime without publishing the
crates or tracking an external version.

tom is a Rust port of pi, and is MIT licensed. The upstream `LICENSE` is
reproduced alongside this file. Terry ships it as GPL-3.0-or-later, which MIT
permits; keep the copyright notice when redistributing.

Synced from tom at commit `2c877ee` (2026-11-02).

Upstream crate: `crates/` + this directory name. Local adaptations:

- Kept on Rust edition 2021 (Terry's workspace is edition 2024). The code
  predates edition 2024's `unsafe` requirement on `std::env::set_var`.
- Dependencies are re-pointed at Terry's workspace dependencies so the two
  dependency graphs share versions (`rusqlite` is bumped to 0.32 to match the
  `libsqlite3-sys` version `sqlez` already links).

Do not edit the sources here for Terry-specific behaviour. Put integration code
in `crates/agent_embed` instead, so re-vendoring stays a copy.
