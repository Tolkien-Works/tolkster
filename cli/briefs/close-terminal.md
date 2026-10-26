`echo '<line>'; [ -n "$ORCA_TERMINAL_HANDLE" ] && orca terminal close --terminal "$ORCA_TERMINAL_HANDLE" --json`,
   or in PowerShell `echo '<line>'; if ($env:ORCA_TERMINAL_HANDLE) { orca terminal close --terminal $env:ORCA_TERMINAL_HANDLE --json }`,
   as your last command, with no turn of its own to read the handle first. With
   `ORCA_TERMINAL_HANDLE` empty it closes nothing: an empty `--terminal` closes the
   active terminal, which may be someone else's.
