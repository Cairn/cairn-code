// VHS tape file for generating Cairn Code screenshots
// Install vhs: go install github.com/charmbracelet/vhs@latest
// Run: vhs screenshots/repl-startup.tape

Output screenshots/repl-startup.png
Set FontSize 16
Set Width 1200
Set Height 500
Set Theme "Catppuccin Mocha"

Type "./cairn-code"
Enter
Wait 1s
Screenshot