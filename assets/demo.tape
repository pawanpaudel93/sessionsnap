# The README's demo (assets/demo.gif). Record with: vhs assets/demo.tape
# It runs in a throwaway world: stand-in agents, its own HOME and tmux server. SETUP is a script
# that builds that world and starts tmux with two projects whose agents are running. Every command
# shown must only see that world (its tmux server), never the machine's own processes.
Output assets/demo.gif
Set Shell "bash"
Set FontSize 18
Set Width 1100
Set Height 640
Set Padding 18
Set Theme "Catppuccin Mocha"
Set TypingSpeed 45ms

Hide
Type `. "$SETUP" && tmux attach -t home` Enter
Sleep 2.5s
Type "clear" Enter
Sleep 500ms
Show

Type "sessionsnap save work" Sleep 400ms Enter Sleep 2s
Type "sessionsnap show work" Sleep 400ms Enter Sleep 3.5s
Type "clear" Enter
Type "# a crash, a reboot, a closed window..." Enter Sleep 600ms
Type "tmux kill-session -t api; tmux kill-session -t web" Sleep 300ms Enter Sleep 1s
Type "tmux ls" Enter Sleep 1.5s
Type "sessionsnap restore work --yes" Sleep 400ms Enter Sleep 2.5s
Type "tmux ls" Enter Sleep 1.5s
Type "tmux capture-pane -pt api:0 | head -1; tmux capture-pane -pt web:0 | head -1" Sleep 300ms Enter Sleep 4s
