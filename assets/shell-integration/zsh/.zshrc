# Terry shell integration shim.
#
# Restore the user's ZDOTDIR so their configuration keeps seeing its real
# location, then source their .zshrc (exactly one of $ZDOTDIR/.zshrc or
# $HOME/.zshrc, so it is never loaded twice), and finally load Terry's
# integration hooks last, so prompt marks (OSC 133) and working directory
# reports (OSC 7) are emitted no matter what the user configures.

if [ -n "$TERRY_USER_ZDOTDIR" ]; then
  ZDOTDIR="$TERRY_USER_ZDOTDIR"
  if [ -f "$ZDOTDIR/.zshrc" ]; then
    . "$ZDOTDIR/.zshrc"
  fi
else
  unset ZDOTDIR
  if [ -f "$HOME/.zshrc" ]; then
    . "$HOME/.zshrc"
  fi
fi

if [ -n "$TERRY_INTEGRATION_DIR" ] && [ -r "$TERRY_INTEGRATION_DIR/zsh/terry.zsh" ]; then
  . "$TERRY_INTEGRATION_DIR/zsh/terry.zsh"
fi
