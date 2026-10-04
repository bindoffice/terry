# Terry shell integration.
#
# Emits OSC 133 (prompt marks) and OSC 7 (working directory) sequences so Terry
# can navigate between prompts (ctrl-shift-up / ctrl-shift-down) and track each
# prompt's directory. Loaded automatically by Terry; safe to source manually.

if [[ -n "$TERRY_SHELL_INTEGRATION_LOADED" ]]; then
  return 0
fi
TERRY_SHELL_INTEGRATION_LOADED=1

# Runs before each prompt is drawn. "A" opens the prompt mark at the cursor
# (the exact start of the prompt line) and "B" closes it. "D" reports the
# previous command's exit status, and OSC 7 reports the working directory.
__terry_precmd() {
  local __terry_exit_code=$?
  printf '\e]133;D;%s\e\\' "$__terry_exit_code"
  printf '\e]7;%s\e\\' "$PWD"
  printf '\e]133;A\e\\'
  printf '\e]133;B\e\\'
}

# Runs before an interactive command line is executed.
__terry_preexec() {
  printf '\e]133;C\e\\'
}

if [[ -o interactive ]]; then
  autoload -Uz add-zsh-hook
  add-zsh-hook precmd __terry_precmd
  add-zsh-hook preexec __terry_preexec
fi
