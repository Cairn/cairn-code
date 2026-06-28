//go:build windows

package main

import "github.com/cairn/cairn-code/internal/tools"

func registerShellTool(r *tools.Registry) {
	r.Register(tools.NewPowerShellTool())
}
