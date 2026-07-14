# scripts/measure-resources.ps1
# Script to measure Jello application memory footprint (including WebView2 child processes).

$processName = "jello"
$budgetMb = 80.0

Write-Host "Looking for process '$processName'..." -ForegroundColor Cyan

$mainProcesses = Get-Process -Name $processName -ErrorAction SilentlyContinue

if (-not $mainProcesses) {
    Write-Warning "Process '$processName' is not currently running. Please start Jello first."
    Exit 1
}

foreach ($mainProc in $mainProcesses) {
    $parentPid = $mainProc.Id
    Write-Host "Found main process PID: $parentPid" -ForegroundColor Green

    # Recursive function to get all child processes
    function Get-ChildProcesses($procId) {
        $children = Get-CimInstance Win32_Process -Filter "ParentProcessId = $procId" -ErrorAction SilentlyContinue
        $results = @()
        foreach ($child in $children) {
            $childProc = Get-Process -Id $child.ProcessId -ErrorAction SilentlyContinue
            if ($childProc) {
                $results += $childProc
                $results += Get-ChildProcesses($child.ProcessId)
            }
        }
        return $results
    }

    $childProcs = Get-ChildProcesses $parentPid
    $allProcs = @($mainProc) + $childProcs

    Write-Host "`nProcess Tree and Memory Usage:" -ForegroundColor Yellow
    Write-Host ("{0,-30} {1,-10} {2,15}" -f "Process Name", "PID", "WorkingSet (MB)")
    Write-Host ("-" * 60)

    $totalWs = 0
    foreach ($proc in $allProcs) {
        $wsMb = [Math]::Round($proc.WorkingSet64 / 1MB, 2)
        $totalWs += $proc.WorkingSet64
        Write-Host ("{0,-30} {1,-10} {2,15:N2}" -f $proc.ProcessName, $proc.Id, $wsMb)
    }

    $totalWsMb = [Math]::Round($totalWs / 1MB, 2)
    Write-Host ("-" * 60)
    
    if ($totalWsMb -le $budgetMb) {
        Write-Host ("TOTAL MEMORY FOOTPRINT: {0:N2} MB (BUDGET: {1} MB) - PASS" -f $totalWsMb, $budgetMb) -ForegroundColor Green
    } else {
        Write-Host ("TOTAL MEMORY FOOTPRINT: {0:N2} MB (BUDGET: {1} MB) - FAIL" -f $totalWsMb, $budgetMb) -ForegroundColor Red
    }
    Write-Host ""
}
