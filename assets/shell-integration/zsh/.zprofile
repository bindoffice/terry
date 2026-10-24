# Terry shell integration shim for login shells.
#
# On macOS Terry starts zsh through /usr/bin/login, so zsh reads
# $ZDOTDIR/.zprofile with ZDOTDIR pointing at the integration directory. Source
# the user's real .zprofile here, then point ZDOTDIR back at the shim so the
# matching .zshrc shim still runs and enables Terry's prompt integration.

if [ -n "$TERRY_USER_ZDOTDIR" ]; then
  ZDOTDIR="$TERRY_USER_ZDOTDIR"
  if [ -f "$ZDOTDIR/.zprofile" ]; then
    . "$ZDOTDIR/.zprofile"
  fi
elif [ -f "$HOME/.zprofile" ]; then
  . "$HOME/.zprofile"
fi

if [ -n "$TERRY_INTEGRATION_DIR" ]; then
  ZDOTDIR="$TERRY_INTEGRATION_DIR/zsh"
fi
