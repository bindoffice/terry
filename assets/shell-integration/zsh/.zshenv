# Terry shell integration shim.
#
# Terry starts zsh with ZDOTDIR pointing at this directory so it can chain-load
# your own configuration. This file sources your real ~/.zshenv (or
# $ZDOTDIR/.zshenv, whichever was in effect); the matching .zshrc shim restores
# ZDOTDIR, sources your .zshrc, and then enables Terry's prompt integration.

if [ -n "$TERRY_USER_ZDOTDIR" ]; then
  if [ -f "$TERRY_USER_ZDOTDIR/.zshenv" ]; then
    . "$TERRY_USER_ZDOTDIR/.zshenv"
  fi
elif [ -f "$HOME/.zshenv" ]; then
  . "$HOME/.zshenv"
fi
