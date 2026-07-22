Option Explicit

Dim shell, fso, repoRoot, exePath, installedPath, command

Set shell = CreateObject("WScript.Shell")
Set fso = CreateObject("Scripting.FileSystemObject")

repoRoot = fso.GetParentFolderName(WScript.ScriptFullName)
exePath = fso.BuildPath(repoRoot, "target\release\LumiControl.exe")

If Not fso.FileExists(exePath) Then
    exePath = fso.BuildPath(repoRoot, "target\release\lumi-ui.exe")
End If

If Not fso.FileExists(exePath) Then
    installedPath = shell.ExpandEnvironmentStrings("%LOCALAPPDATA%\LumiControl\LumiControl.exe")
    exePath = installedPath
End If

If Not fso.FileExists(exePath) Then
    MsgBox "LumiControl V2 executable not found:" & vbCrLf & exePath, vbCritical, "LumiControl"
    WScript.Quit 1
End If

shell.CurrentDirectory = repoRoot
command = Chr(34) & exePath & Chr(34)
shell.Run command, 0, False
