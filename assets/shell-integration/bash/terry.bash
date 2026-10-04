# Terry shell integration (bash, manual).
#
# Terry exports $TERRY_INTEGRATION_DIR for bash but cannot load scripts on its
# own, so add this line to your ~/.bashrc:
#
#   [ -n "$TERRY_INTEGRATION_DIR" ] && . "$TERRY_INTEGRATION_DIR/bash/terry.bash"
#
# Emits OSC 133 (prompt marks) and OSC 7 (working directory) sequences so Terry
# can navigate between prompts and track each prompt's directory. Requires
# bash >= 4 (precmd_functions / preexec_functions).

case "$-" in
  *i*) ;;
  *) return 0 2>/dev/null || exit 0 ;;
esac

if [ -n "$TERRY_SHELL_INTEGRATION_LOADED" ]; then
  return 0 2>/dev/null || exit 0
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

case ":${precmd_functions[*]}:" in
  *":__terry_precmd:"*) ;;
  *) precmd_functions+=(__terry_precmd) ;;
esac

case ":${preexec_functions[*]}:" in
  *":__terry_preexec:"*) ;;
  *) preexec_functions+=(__terry_preexec) ;;
esac
