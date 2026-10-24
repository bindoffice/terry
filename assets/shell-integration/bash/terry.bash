# Terry shell integration (bash, manual).
#
# Terry exports $TERRY_INTEGRATION_DIR for bash but cannot load scripts on its
# own, so add this line to your ~/.bashrc:
#
#   [ -n "$TERRY_INTEGRATION_DIR" ] && . "$TERRY_INTEGRATION_DIR/bash/terry.bash"
#
# Emits OSC 133 (prompt marks) and OSC 7 (working directory) sequences so Terry
# can navigate between prompts, copy command output, and track each prompt's
# directory.

case "$-" in
  *i*) ;;
  *) return 0 2>/dev/null || exit 0 ;;
esac

if [ -n "$TERRY_SHELL_INTEGRATION_LOADED" ]; then
  return 0 2>/dev/null || exit 0
fi
TERRY_SHELL_INTEGRATION_LOADED=1

# Runs before each prompt is drawn. "D" reports the previous command's exit
# status and OSC 7 reports the working directory. The "A"/"B" prompt marks are
# emitted by __terry_prompt_start, after all other PROMPT_COMMAND entries have
# run, so a pre-existing prompt hook cannot interleave with them.
__terry_precmd() {
  local __terry_exit_code=$?
  printf '\e]133;D;%s\e\\' "$__terry_exit_code"
  # OSC 7 wants a file:// URI; encode a literal '%' so the terminal's
  # percent-decoder does not mis-read it.
  printf '\e]7;file://%s%s\e\\' "${HOSTNAME:-localhost}" "${PWD//\%/%25}"
}

# Runs as the last PROMPT_COMMAND entry: mark the prompt and remember that the
# next accepted command line should emit "C".
__terry_prompt_start() {
  printf '\e]133;A\e\\'
  printf '\e]133;B\e\\'
  __terry_at_prompt=1
}

# Runs before an interactive command line is executed. The DEBUG trap cannot
# tell an empty-line Enter from a real command, so only the transition from
# "prompt shown" to "command starts" emits "C" (once per command line).
__terry_preexec() {
  [ "${__terry_at_prompt:-0}" = 1 ] || return 0
  __terry_at_prompt=0
  case "$BASH_COMMAND" in
    *__terry_precmd*) return 0 ;;
  esac
  printf '\e]133;C\e\\'
}

# bash-preexec (https://github.com/rcaloras/bash-preexec) is widely used and
# already owns the DEBUG trap. Hook into its arrays instead of fighting it.
if declare -F __bp_precmd_invoke_cmd >/dev/null 2>&1 \
  && [ "${preexec_functions+set}" = set ] \
  && [ "${precmd_functions+set}" = set ]; then
  # `__terry_precmd` must run first (it captures the command's exit status) and
  # `__terry_prompt_start` last (right before the prompt is drawn).
  precmd_functions=(__terry_precmd "${precmd_functions[@]}" __terry_prompt_start)
  preexec_functions+=(__terry_preexec)
  return 0 2>/dev/null || exit 0
fi

# A foreign DEBUG trap (e.g. from a prompt framework that does not use
# bash-preexec) is left untouched: overwriting it would break that framework.
# We still get prompt marks and cwd tracking from PROMPT_COMMAND.
if [ -n "$(trap -p DEBUG)" ]; then
  __terry_install_debug_trap=0
else
  __terry_install_debug_trap=1
fi

# PROMPT_COMMAND may be a scalar (bash < 5.1) or an array (bash >= 5.1).
# Prepend the status/cwd hook and append the prompt-mark hook so user hooks run
# in between without seeing the "at prompt" flag too early.
__terry_pc_decl=$(declare -p PROMPT_COMMAND 2>/dev/null || true)
case "$__terry_pc_decl" in
  "declare -a"*)
    case "${PROMPT_COMMAND[*]}" in
      *__terry_precmd*) ;;
      *)
        PROMPT_COMMAND=(__terry_precmd "${PROMPT_COMMAND[@]}" __terry_prompt_start)
        ;;
    esac
    ;;
  *)
    case ";$PROMPT_COMMAND;" in
      *";__terry_precmd;"*) ;;
      "")
        PROMPT_COMMAND="__terry_precmd;__terry_prompt_start"
        ;;
      *)
        PROMPT_COMMAND="__terry_precmd;$PROMPT_COMMAND;__terry_prompt_start"
        ;;
    esac
    ;;
esac
unset __terry_pc_decl

if [ "$__terry_install_debug_trap" = 1 ]; then
  trap '__terry_preexec' DEBUG
fi
unset __terry_install_debug_trap
