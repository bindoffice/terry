# Terry shell integration.
#
# Terry prepends this directory to XDG_DATA_DIRS so fish loads this file from
# vendor_conf.d on startup. Emits OSC 133 (prompt marks) and OSC 7 (working
# directory) sequences so Terry can navigate between prompts and track each
# prompt's directory.

if not set -q TERRY_SHELL_INTEGRATION_LOADED
    set -g TERRY_SHELL_INTEGRATION_LOADED 1

    # Runs before each prompt is drawn. "A" opens the prompt mark at the
    # cursor (the exact start of the prompt line) and "B" closes it. "D"
    # reports the previous command's exit status, and OSC 7 reports the
    # working directory.
    function __terry_fish_precmd --on-event fish_prompt
        set -l exit_code $status
        printf '\e]133;D;%s\e\\' $exit_code
        printf '\e]7;%s\e\\' $PWD
        printf '\e]133;A\e\\'
        printf '\e]133;B\e\\'
    end

    # Runs before an interactive command line is executed.
    function __terry_fish_preexec --on-event fish_preexec
        printf '\e]133;C\e\\'
    end
end
