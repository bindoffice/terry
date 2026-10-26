# Terry shell integration (PowerShell).
#
# PowerShell has no environment hook that runs after the profile is loaded, so
# Terry types a dot-source command into the freshly spawned shell. This script
# wraps the prompt function and PSReadLine's history handler to emit OSC 133
# (prompt marks) and OSC 7 (working directory) sequences, which power prompt
# navigation, "copy last command output", and per-prompt cwd tracking.

if ($env:TERRY_SHELL_INTEGRATION_LOADED -eq '1') {
    return
}
$env:TERRY_SHELL_INTEGRATION_LOADED = '1'

$global:__terry_esc = [string][char]27
$global:__terry_st = $global:__terry_esc + '\'

function global:__terry_write([string]$Sequence) {
    # `[Console]::Write` bypasses the output pipeline, so the sequences are not
    # captured by $(...) or redirected to a variable.
    [Console]::Write($Sequence)
}

# OSC 7 wants a `file://` URI. Windows paths use backslashes and start with a
# drive letter, so convert to forward slashes and prefix a leading "/".
function global:__terry_cwd_uri() {
    $path = $PWD.ProviderPath
    if (-not $path) {
        return $null
    }
    $path = $path.Replace('\', '/')
    # A literal '%' must be encoded or the terminal's percent-decoder
    # mis-reads the path.
    $path = $path.Replace('%', '%25')
    if (-not $path.StartsWith('/')) {
        $path = '/' + $path
    }
    $hostName = $env:COMPUTERNAME
    if (-not $hostName) {
        $hostName = 'localhost'
    }
    return "file://$hostName$path"
}

# Remember the user's prompt function once. Re-sourcing this script (or a
# profile that defines `prompt` afterwards) must not wrap it recursively.
if (-not (Get-Command __terry_original_prompt -CommandType Function -ErrorAction SilentlyContinue)) {
    $terryPromptCommand = Get-Command prompt -CommandType Function -ErrorAction SilentlyContinue
    if ($null -ne $terryPromptCommand -and $null -ne $terryPromptCommand.ScriptBlock) {
        Set-Item -Path function:global:__terry_original_prompt -Value $terryPromptCommand.ScriptBlock
    } else {
        # Mirror PowerShell's default prompt so a shell without one still works.
        function global:__terry_original_prompt {
            "PS $($executionContext.SessionState.Path.CurrentLocation)$('>' * ($nestedPromptLevel + 1)) "
        }
    }
}

function global:prompt {
    # Capture the previous command's status before anything else runs: `$?`
    # reflects the last command only until the next statement executes.
    $terrySuccess = $?
    $terryNativeExit = $LASTEXITCODE
    $esc = $global:__terry_esc
    $st = $global:__terry_st

    $terryCode = 0
    if (-not $terrySuccess) {
        if ($null -ne $terryNativeExit -and $terryNativeExit -gt 0) {
            $terryCode = $terryNativeExit
        } else {
            $terryCode = 1
        }
    }

    __terry_write "$esc]133;D;$terryCode$st"

    $terryCwd = __terry_cwd_uri
    if ($terryCwd) {
        __terry_write "$esc]7;$terryCwd$st"
    }

    $terryPrompt = [string](__terry_original_prompt)

    # "A" marks the start of the prompt and "B" its end, which is also where
    # the user's command line begins. They surround the prompt text so the
    # terminal records the prompt's full width.
    __terry_write "$esc]133;A$st"
    __terry_write $terryPrompt
    __terry_write "$esc]133;B$st"

    # The prompt was already written, so return an empty string to keep
    # PowerShell from writing it a second time.
    return ''
}

# Emit "C" (command start) just before PSReadLine hands the line to the engine.
# The user's handler is chained rather than replaced so their configuration
# (for example history filtering) keeps working.
if (Get-Module -Name PSReadLine) {
    try {
        $terryOriginalHandler = (Get-PSReadLineOption).AddToHistoryHandler
        $terryHandler = {
            param([string]$Line)

            $esc = $global:__terry_esc
            $st = $global:__terry_st

            $terryAdd = $true
            if ($null -ne $terryOriginalHandler) {
                $terryAdd = & $terryOriginalHandler $Line
            }
            if ($null -eq $terryAdd) {
                $terryAdd = $true
            }

            # The command text is base64-encoded in the "C" payload so Terry
            # can re-run it later. Long lines are sent without a payload.
            if ($Line -and $Line.Length -le 128) {
                $terryEncoded = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($Line))
                __terry_write "$esc]133;C;$terryEncoded$st"
            } else {
                __terry_write "$esc]133;C$st"
            }

            return $terryAdd
        }
        Set-PSReadLineOption -AddToHistoryHandler ($terryHandler.GetNewClosure())
    } catch {
        # PSReadLine may be unavailable (for example `powershell -NonInteractive`)
        # or the option may be unsupported. Prompt marks still work without "C".
    }
}
