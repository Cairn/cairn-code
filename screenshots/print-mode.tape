// VHS tape for print-mode screenshot
// Run: vhs screenshots/print-mode.tape

Output screenshots/print-mode.png
Set FontSize 16
Set Width 1200
Set Height 500
Set Theme "Catppuccin Mocha"

Type "./cairn-code -p \"find all go files, count lines of code\""
Enter
Wait 15s
Screenshot