# Terry shell integration shim.
#
# Terry starts zsh with ZDOTDIR pointing at this directory so it can chain-load
# your own configuration. This file sources your real .zshenv (or
# $ZDOTDIR/.zshenv, whichever was in effect) with ZDOTDIR temporarily restored,
# then points ZDOTDIR back at the shim so the matching .zprofile / .zshrc shims
# still run.

if [ -n "$TERRY_USER_ZDOTDIR" ]; then
  ZDOTDIR="$TERRY_USER_ZDOTDIR"
  if [ -f "$ZDOTDIR/.zshenv" ]; then
    . "$ZDOTDIR/.zshenv"
  fi
else
  unset ZDOTDIR
  if [ -f "$HOME/.zshenv" ]; then
    . "$HOME/.zshenv"
    # A .zshenv may set ZDOTDIR itself; remember it so the .zshrc shim restores
    # the same value instead of falling back to $HOME.
    if [ -n "$ZDOTDIR" ] && [ "$ZDOTDIR" != "$TERRY_INTEGRATION_DIR/zsh" ]; then
      TERRY_USER_ZDOTDIR="$ZDOTDIR"
      export TERRY_USER_ZDOTDIR
    fi
  fi
fi

if [ -n "$TERRY_INTEGRATION_DIR" ]; then
  ZDOTDIR="$TERRY_INTEGRATION_DIR/zsh"
fi
