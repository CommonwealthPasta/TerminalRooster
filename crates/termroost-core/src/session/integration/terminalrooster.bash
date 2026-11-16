# TerminalRooster shell integration for bash.
#
# When a command typed at the prompt starts, tells the terminal what runs:
# OSC 633;E with the command, then OSC 133;C. After it, tells how it ended:
# OSC 633;E with the command line, then OSC 133;D with the exit status.
# When the working directory changes, reports it with OSC 633;P;Cwd=<dir>.
# TerminalRooster starts bash with PROMPT_COMMAND set to source this file on
# the first prompt (after ~/.bashrc has run); it then takes that entry out of
# PROMPT_COMMAND again and runs whatever else ~/.bashrc put there.

if [[ -z "${__terminalrooster_loaded-}" && $- == *i* ]]; then
    __terminalrooster_loaded=1
    # Not for programs started from this shell.
    export -n PROMPT_COMMAND

    # The user's own prompt command, minus the entry that sourced this file
    # (~/.bashrc may have added its own before or after it). Since bash 5.1
    # PROMPT_COMMAND can be an array; all of it runs from our hook below.
    __terminalrooster_original="$(IFS=$'\n'; builtin printf '%s' "${PROMPT_COMMAND[*]-}")"
    __terminalrooster_source="${TERMINALROOSTER_BASH_INTEGRATION-}"
    if [[ -n "$__terminalrooster_source" ]]; then
        __terminalrooster_original="${__terminalrooster_original//"$__terminalrooster_source;"/}"
        __terminalrooster_original="${__terminalrooster_original//"$__terminalrooster_source"/}"
    fi
    # A separator left at the start would be a syntax error.
    while [[ "$__terminalrooster_original" == [[:space:]\;]* ]]; do
        __terminalrooster_original="${__terminalrooster_original:1}"
    done
    unset TERMINALROOSTER_BASH_INTEGRATION
    __terminalrooster_at_prompt=
    __terminalrooster_running=
    __terminalrooster_command=
    __terminalrooster_escaped=
    __terminalrooster_histcmd="${HISTCMD-}"
    __terminalrooster_pwd=

    # Escapes $1 for an OSC 633 parameter into __terminalrooster_escaped (not
    # printed: capturing output takes a subshell, which is slow in Git Bash).
    __terminalrooster_escape() {
        local s="$1"
        s="${s//\\/\\\\}"
        s="${s//;/\\x3b}"
        s="${s//$'\n'/\\x0a}"
        s="${s//$'\r'/\\x0d}"
        s="${s//$'\a'/}"
        s="${s//$'\e'/}"
        __terminalrooster_escaped="$s"
    }

    __terminalrooster_preexec() {
        # Runs before every simple command; only the first one typed at the
        # prompt counts (not our prompt hook, nor completion).
        if [[ -n "$__terminalrooster_at_prompt" && -z "${COMP_LINE-}" \
            && "$BASH_COMMAND" != __terminalrooster_prompt_command* ]]; then
            __terminalrooster_at_prompt=
            __terminalrooster_running=1
            __terminalrooster_command="$BASH_COMMAND"
            # Git Bash's own tools (vim, less, ssh) can't be seen running
            # from outside, so say what runs.
            __terminalrooster_escape "$BASH_COMMAND"
            builtin printf '\e]633;E;%s\a\e]133;C\a' "$__terminalrooster_escaped"
        fi
    }

    __terminalrooster_prompt_command() {
        local status=$?
        __terminalrooster_at_prompt=
        # A command ran if the DEBUG trap saw it, or if it went into the
        # history: a lone subshell like `(make)` runs no simple command here.
        if [[ -n "$__terminalrooster_running" || "${HISTCMD-}" != "$__terminalrooster_histcmd" ]]; then
            __terminalrooster_running=
            local line=
            if [[ $status -ne 0 ]]; then
                line="$(HISTTIMEFORMAT='' builtin history 1 2>/dev/null)"
                # " 123  cargo test" -> "cargo test"
                [[ "$line" =~ ^[[:space:]]*[0-9]+[[:space:]]+(.*)$ ]] && line="${BASH_REMATCH[1]}"
                [[ -z "$line" ]] && line="$__terminalrooster_command"
            fi
            __terminalrooster_escape "$line"
            builtin printf '\e]633;E;%s\a\e]133;D;%s\a' "$__terminalrooster_escaped" "$status"
        fi
        __terminalrooster_histcmd="${HISTCMD-}"
        # Where the shell is, so the terminal starts there after a restart.
        if [[ "$PWD" != "$__terminalrooster_pwd" ]]; then
            __terminalrooster_pwd="$PWD"
            local dir="$PWD"
            # Git Bash and MSYS2 name drives /c/...; `pwd -W` gives the
            # Windows path (it costs a subshell, hence only on a change).
            if [[ "$OSTYPE" == msys* || "$OSTYPE" == cygwin* ]]; then
                dir="$(builtin pwd -W 2>/dev/null)" || dir="$PWD"
            fi
            __terminalrooster_escape "$dir"
            builtin printf '\e]633;P;Cwd=%s\a' "$__terminalrooster_escaped"
        fi
        if [[ -n "$__terminalrooster_original" ]]; then
            (exit "$status")
            builtin eval "$__terminalrooster_original"
        fi
        __terminalrooster_at_prompt=1
        return "$status"
    }

    # Keep a DEBUG trap the user already had. `trap -p` prints it quoted, as
    # `trap -- '<command>' DEBUG`; let bash itself undo the quoting.
    __terminalrooster_debug=
    __terminalrooster_capture() { __terminalrooster_debug="$2"; }
    __terminalrooster_trap="$(builtin trap -p DEBUG)"
    if [[ -n "$__terminalrooster_trap" ]]; then
        builtin eval "__terminalrooster_capture ${__terminalrooster_trap#trap }"
    fi
    unset -f __terminalrooster_capture
    unset __terminalrooster_trap
    if [[ -n "$__terminalrooster_debug" ]]; then
        builtin trap '__terminalrooster_preexec; builtin eval "$__terminalrooster_debug"' DEBUG
    else
        builtin trap '__terminalrooster_preexec' DEBUG
    fi
    PROMPT_COMMAND=(__terminalrooster_prompt_command)
    # The first prompt is up now.
    __terminalrooster_at_prompt=1
fi
