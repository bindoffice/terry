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
- No hardcoded dark/black surfaces. Inline-code and code-block backgrounds used
  to be fixed One Dark greys, and the progress bar, scrollbar thumb and image
  placeholder pinned ANSI black/bright-black. All of them now come from the
  theme: `ThemeColors::md_code_bg` / `md_code_block_bg` fall back to the
  theme's `surface`, and `mdCodeBg` / `mdCodeBlockBg` are accepted as theme
  tokens so a host palette (Terry writes one per terminal) can set them.

Do not edit the sources here for Terry-specific behaviour. Put integration code
in the host application instead, so re-vendoring stays a copy.
