// VHS tape for agent-action screenshot
// Shows the agent autonomously using tools
// Run: vhs screenshots/agent-action.tape

Output screenshots/agent-action.png
Set FontSize 16
Set Width 1200
Set Height 700
Set Theme "Catppuccin Mocha"

Type "./cairn-code -p \"read the file CAIRN.md and list all tools\""
Enter
Wait 15s
Screenshot