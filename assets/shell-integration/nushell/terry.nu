# Terry shell integration (Nushell).
#
# Nushell has no environment hook that runs after config.nu is loaded, so Terry
# types a `source` command into the freshly spawned shell. This script appends
# hooks that emit OSC 133 (prompt marks) and OSC 7 (working directory)
# sequences, which power prompt navigation, "copy last command output", and
# per-prompt cwd tracking.
#
# Hook signatures have changed across Nushell releases, so the pre-execution
# closure accepts an optional command argument: it is the command string on
# versions that pass one, and null on versions that do not.

# Writes an OSC sequence: ESC ] <body> ESC \
def __terry_osc [body: string] {
    print -n $"(char esc)]($body)(char esc)\\"
}

# Runs before each prompt is drawn. "D" reports the previous command's exit
# status, OSC 7 reports the working directory, and "A"/"B" bracket the prompt.
def __terry_pre_prompt [] {
    let code = ($env.LAST_EXIT_CODE? | default 0)
    let host = ($env.HOSTNAME? | default 'localhost')
    # A literal '%' must be encoded or the terminal's percent-decoder
    # mis-reads the path.
    let cwd = ($env.PWD | path expand | into string | str replace --all '%' '%25')
    __terry_osc $"133;D;($code)"
    __terry_osc $"7;file://($host)($cwd)"
    __terry_osc "133;A"
    __terry_osc "133;B"
}

# Runs before an interactive command line is executed. The command text is
# base64-encoded in the "C" payload so Terry can re-run it later; long lines
# are sent without a payload.
def __terry_pre_exec [command?: string] {
    let line = ($command | default '')
    let length = ($line | str length)
    if $length == 0 {
        __terry_osc "133;C"
    } else if $length <= 128 {
        let encoded = ($line | encode base64)
        __terry_osc $"133;C;($encoded)"
    } else {
        __terry_osc "133;C"
    }
}

# The guard keeps re-sourcing from installing the hooks twice. Hooks the user
# already configured are preserved; ours run last so the marks land right
# before the prompt and right before execution.
if not ("TERRY_SHELL_INTEGRATION_LOADED" in $env) {
    $env.TERRY_SHELL_INTEGRATION_LOADED = '1'
    $env.config.hooks = ($env.config.hooks | default {})
    $env.config.hooks.pre_prompt = (($env.config.hooks.pre_prompt? | default []) | append { __terry_pre_prompt })
    $env.config.hooks.pre_execution = (($env.config.hooks.pre_execution? | default []) | append {|command?| __terry_pre_exec $command})
}
