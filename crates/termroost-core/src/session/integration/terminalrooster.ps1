# TerminalRooster shell integration for PowerShell.
#
# After each command typed at the prompt, tells the terminal how it ended:
# OSC 633;E with the command line, then OSC 133;D with the exit status.
# When the working directory changes, reports it with OSC 633;P;Cwd=<dir>.
# TerminalRooster runs this after the profile, so it wraps whatever prompt
# function the profile set up (oh-my-posh, starship, ...).

if (-not $global:__TerminalRoosterLoaded) {
    $global:__TerminalRoosterLoaded = $true
    $global:__TerminalRoosterLastId = (Get-History -Count 1).Id
    $global:__TerminalRoosterPrompt = $function:prompt
    $global:__TerminalRoosterCwd = $null

    function global:prompt {
        # Must come first: anything else run here changes them.
        $ok = $global:?
        $code = $global:LASTEXITCODE
        $esc = [char]27
        $bel = [char]7
        # Where the shell is, so the terminal starts there after a restart
        # (not in a registry drive or the like).
        $here = $executionContext.SessionState.Path.CurrentLocation
        if ($here.Provider.Name -eq 'FileSystem' -and $here.ProviderPath -ne $global:__TerminalRoosterCwd) {
            $global:__TerminalRoosterCwd = $here.ProviderPath
            $dir = $here.ProviderPath -replace '\\', '\\' -replace ';', '\x3b'
            [Console]::Write("$esc]633;P;Cwd=$dir$bel")
        }
        $last = Get-History -Count 1
        # No new history entry: an empty line or Ctrl+C at the prompt.
        if ($last -and $last.Id -ne $global:__TerminalRoosterLastId) {
            $global:__TerminalRoosterLastId = $last.Id
            $status = if ($ok) { 0 } elseif ($code -is [int] -and $code -ne 0) { $code } else { 1 }
            $line = ''
            if ($status -ne 0) {
                $line = $last.CommandLine -replace '\\', '\\' -replace ';', '\x3b' -replace "`n", '\x0a' -replace "`r", '\x0d'
                $line = $line -replace "[\x00-\x1f]", ''
            }
            [Console]::Write("$esc]633;E;$line$bel$esc]133;D;$status$bel")
        }
        $global:LASTEXITCODE = $code
        if ($global:__TerminalRoosterPrompt) { & $global:__TerminalRoosterPrompt } else { "PS $($executionContext.SessionState.Path.CurrentLocation)> " }
    }
}
