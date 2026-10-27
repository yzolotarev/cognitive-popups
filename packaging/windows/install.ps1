# User-local installation. Does not register global hotkeys or touch old data.
param([switch]$Autostart)
$ErrorActionPreference = 'Stop'
$destination = Join-Path $env:LOCALAPPDATA 'CognitiveExoskeletonV1'
New-Item -ItemType Directory -Force -Path $destination | Out-Null
$executable = Join-Path $destination 'cognitive-exoskeleton.exe'
Copy-Item (Join-Path $PSScriptRoot 'cognitive-exoskeleton.exe') $executable -Force
$shell = New-Object -ComObject WScript.Shell
$shortcut = $shell.CreateShortcut((Join-Path ([Environment]::GetFolderPath('Programs')) 'Cognitive Exoskeleton.lnk'))
$shortcut.TargetPath = $executable
$shortcut.Arguments = 'hub'
$shortcut.Save()
if ($Autostart) {
    $startup = $shell.CreateShortcut((Join-Path ([Environment]::GetFolderPath('Startup')) 'Cognitive Exoskeleton.lnk'))
    $startup.TargetPath = $executable
    $startup.Arguments = 'hub'
    $startup.Save()
}
Write-Output "Installed: $executable. Autostart is opt-in. Desktop acceptance remains pending."
