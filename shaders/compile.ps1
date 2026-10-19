# Refreshes the precompiled shader fallback in shaders/compiled/ (used by build.rs
# only when fxc.exe is not available). Run after editing any *.hlsl.
$ErrorActionPreference = 'Stop'
$fxc = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\fxc.exe" |
    Sort-Object FullName | Select-Object -Last 1 -ExpandProperty FullName
$dir = $PSScriptRoot
& $fxc /nologo /O3 /Qstrip_reflect /Qstrip_debug /E VSMain /T vs_5_0 /Fo "$dir\compiled\image_vs.cso" "$dir\image.hlsl"
& $fxc /nologo /O3 /Qstrip_reflect /Qstrip_debug /E PSMain /T ps_5_0 /Fo "$dir\compiled\image_ps.cso" "$dir\image.hlsl"
& $fxc /nologo /O3 /Qstrip_reflect /Qstrip_debug /E VSMain /T vs_5_0 /Fo "$dir\compiled\model_vs.cso" "$dir\model.hlsl"
& $fxc /nologo /O3 /Qstrip_reflect /Qstrip_debug /E PSMain /T ps_5_0 /Fo "$dir\compiled\model_ps.cso" "$dir\model.hlsl"
