# Builds target\notes.apk with the shared kit build (see ..\heartnet-kit\build-apk.ps1).
param([switch]$SkipRust)
& "$PSScriptRoot\..\..\tools\build\build-apk.ps1" -AppDir $PSScriptRoot -Lib notes -SkipRust:$SkipRust
