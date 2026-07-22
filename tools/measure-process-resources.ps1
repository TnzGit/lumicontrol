[CmdletBinding()]
param(
    [string]$ProcessName = "screen-brightness",
    [int]$ProcessId = 0,
    [ValidateRange(5, 3600)]
    [int]$DurationSeconds = 60,
    [ValidateRange(1, 60)]
    [int]$IntervalSeconds = 1,
    [switch]$AsJson
)

$ErrorActionPreference = "Stop"

if ($ProcessId -gt 0) {
    $process = Get-Process -Id $ProcessId
} else {
    $process = Get-Process -Name $ProcessName |
        Sort-Object StartTime -Descending |
        Select-Object -First 1
}

if ($null -eq $process) {
    throw "Process '$ProcessName' was not found."
}

$targetId = $process.Id
$startedAt = Get-Date
$lastSampleAt = $startedAt
$lastCpuSeconds = $process.TotalProcessorTime.TotalSeconds
$samples = [System.Collections.Generic.List[object]]::new()

while (((Get-Date) - $startedAt).TotalSeconds -lt $DurationSeconds) {
    Start-Sleep -Seconds $IntervalSeconds
    $now = Get-Date
    $process = Get-Process -Id $targetId
    $cpuSeconds = $process.TotalProcessorTime.TotalSeconds
    $elapsedSeconds = ($now - $lastSampleAt).TotalSeconds
    $cpuPercent = if ($elapsedSeconds -gt 0) {
        (($cpuSeconds - $lastCpuSeconds) / $elapsedSeconds) * 100.0
    } else {
        0.0
    }

    $samples.Add([pscustomobject]@{
        Timestamp = $now.ToString("o")
        CpuPercentOfOneCore = [math]::Round($cpuPercent, 3)
        WorkingSetMB = [math]::Round($process.WorkingSet64 / 1MB, 3)
        PrivateMemoryMB = [math]::Round($process.PrivateMemorySize64 / 1MB, 3)
        Handles = $process.HandleCount
        Threads = $process.Threads.Count
    })

    $lastSampleAt = $now
    $lastCpuSeconds = $cpuSeconds
}

$cpu = $samples | Measure-Object CpuPercentOfOneCore -Average -Maximum
$workingSet = $samples | Measure-Object WorkingSetMB -Average -Maximum
$privateMemory = $samples | Measure-Object PrivateMemoryMB -Average -Maximum
$handles = $samples | Measure-Object Handles -Maximum
$threads = $samples | Measure-Object Threads -Maximum

$summary = [pscustomobject]@{
    ProcessName = $process.ProcessName
    ProcessId = $targetId
    Path = $process.Path
    DurationSeconds = [math]::Round(((Get-Date) - $startedAt).TotalSeconds, 2)
    SampleCount = $samples.Count
    AverageCpuPercentOfOneCore = [math]::Round($cpu.Average, 3)
    PeakCpuPercentOfOneCore = [math]::Round($cpu.Maximum, 3)
    AverageWorkingSetMB = [math]::Round($workingSet.Average, 3)
    PeakWorkingSetMB = [math]::Round($workingSet.Maximum, 3)
    AveragePrivateMemoryMB = [math]::Round($privateMemory.Average, 3)
    PeakPrivateMemoryMB = [math]::Round($privateMemory.Maximum, 3)
    PeakHandles = [int]$handles.Maximum
    PeakThreads = [int]$threads.Maximum
}

if ($AsJson) {
    $summary | ConvertTo-Json
} else {
    $summary
}
