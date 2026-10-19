# launch.ps1 이 띄운 MOA 만 내리고 실행 사본·작업 폴더를 지운다. 증거 폴더는 남긴다.
#
# 사용: pwsh -File stop.ps1 -RunDir <실행 폴더>
param([Parameter(Mandatory)][string]$RunDir)
. (Join-Path $PSScriptRoot 'common.ps1')

$run = Read-Run -RunDir $RunDir
$proc = Get-Process -Id $run.pid -ErrorAction SilentlyContinue
# PID 재사용 대비 — 경로가 우리 사본일 때만 내린다
if ($proc -and $proc.Path -eq $run.exe) {
    # 먼저 창을 닫게 해 종료 저장을 거치게 한다. 트레이 설정이면 숨기만 하므로 기다린 뒤 강제로 내린다
    $null = $proc.CloseMainWindow()
    if (-not $proc.WaitForExit(5000)) { Stop-Process -Id $run.pid -Force }
    Write-Output "STOPPED PID $($run.pid)"
} elseif ($proc) {
    Write-Output "SKIP PID $($run.pid) — 경로가 다르다($($proc.Path)), 남의 프로세스라 건드리지 않는다"
} else {
    Write-Output "PID $($run.pid) 이미 끝남"
}

# 실행 사본 옆 settings.json 은 세션 부수 효과의 증거라 지우기 전에 증거 폴더로 옮긴다
$settings = Join-Path (Split-Path $run.exe) 'settings.json'
if (Test-Path $settings) { Copy-Item $settings (Join-Path $run.evidenceDir 'settings.final.json') -Force }

foreach ($dir in (Split-Path $run.exe), $run.workDir) {
    if (Test-Path $dir) { Remove-Item -Recurse -Force $dir }
}
$raw = Join-Path $run.evidenceDir '.raw'
if (Test-Path $raw) { Remove-Item -Recurse -Force $raw }
Write-Output "EVIDENCE=$($run.evidenceDir)"
