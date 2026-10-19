# 검증용 MOA 를 빌드해 임시 실행 폴더에서 띄운다.
#
# 사용: pwsh -File launch.ps1 -RepoRoot <레포 루트> [-Release] [-RunId <이름>]
# 출력 마지막 줄: RUN_DIR=<실행 폴더>  — 이후 헬퍼는 이 경로를 -RunDir 로 받는다.
#
# 실행 폴더(%TEMP%\verify-moa\<RunId>\) 구성:
#   app\moa.exe      빌드 산출물의 사본. settings.json·known_hosts.json 이 그 옆에 새로 생긴다
#   work\            기능 조작에 쓰는 빈 폴더 (새 폴더 만들기 등의 부수 효과가 여기 떨어진다)
#   evidence\        스크린샷·트리·판정 기록. stop.ps1 이 지우지 않는다
#   run.json         PID·창 id·경로
param(
    [Parameter(Mandatory)][string]$RepoRoot,
    [switch]$Release,
    [string]$RunId = (Get-Date -Format 'yyyyMMdd-HHmmss')
)
. (Join-Path $PSScriptRoot 'common.ps1')

$RepoRoot = (Resolve-Path $RepoRoot).Path
$profileName = if ($Release) { 'release' } else { 'debug' }

# 중복 실행 방지 뮤텍스(Local\MOA-single-instance)가 로그온 세션 하나에 MOA 하나만 허용한다.
# 다른 MOA 가 떠 있으면 새로 띄운 것은 그 창을 깨우고 스스로 끝나, 사용자의 MOA 를 조작하게 된다
$others = Get-Process moa -ErrorAction SilentlyContinue
if ($others) {
    $list = ($others | ForEach-Object { "PID $($_.Id) $($_.Path)" }) -join '; '
    throw "다른 MOA 가 실행 중이라 띄우지 않는다 — $list. 사용자가 그 MOA 를 종료한 뒤 다시 돌린다(이 스크립트는 남의 프로세스를 내리지 않는다)"
}

Push-Location $RepoRoot
try {
    $cargoArgs = @('build')
    if ($Release) { $cargoArgs += '--release' }
    & cargo @cargoArgs
    if ($LASTEXITCODE -ne 0) { throw "cargo $($cargoArgs -join ' ') 실패 (exit $LASTEXITCODE)" }
} finally { Pop-Location }

$builtExe = Join-Path $RepoRoot "target\$profileName\moa.exe"
if (-not (Test-Path $builtExe)) { throw "빌드 산출물이 없다: $builtExe" }

$runDir = Join-Path $env:TEMP "verify-moa\$RunId"
if (Test-Path $runDir) { throw "실행 폴더가 이미 있다: $runDir — 다른 -RunId 를 준다" }
$appDir = Join-Path $runDir 'app'
$workDir = Join-Path $runDir 'work'
$evidenceDir = Join-Path $runDir 'evidence'
New-Item -ItemType Directory -Force $appDir, $workDir, $evidenceDir | Out-Null
$exe = Join-Path $appDir 'moa.exe'
Copy-Item $builtExe $exe

$proc = Start-Process -FilePath $exe -WorkingDirectory $appDir -PassThru
$run = [ordered]@{
    runId = $RunId; runDir = $runDir; repoRoot = $RepoRoot; profile = $profileName
    builtExe = $builtExe; exe = $exe; pid = $proc.Id; windowId = $null
    exeSha256 = (Get-FileHash $exe -Algorithm SHA256).Hash
    workDir = $workDir; evidenceDir = $evidenceDir; startedAt = (Get-Date).ToString('o')
}
# 창을 찾기 전에 먼저 적는다 — 준비 판정이 실패해도 stop.ps1 이 PID 로 내릴 수 있게
$run | ConvertTo-Json | Set-Content (Join-Path $runDir 'run.json') -Encoding UTF8

# 준비 판정: 이 PID 의 제목 있는 주 창이 뜨고, 그 창의 스크린샷이 한 장 잡혀야 한다
$deadline = (Get-Date).AddSeconds(30)
$window = $null
while ((Get-Date) -lt $deadline) {
    if ($proc.HasExited) { throw "MOA 가 창을 띄우기 전에 끝났다 (exit $($proc.ExitCode)) — 다른 MOA 가 떠 있었는지 본다" }
    $window = Find-MainWindow -ProcessId $proc.Id
    if ($window) { break }
    Start-Sleep -Milliseconds 300
}
if (-not $window) { throw "30초 안에 주 창이 뜨지 않았다 (PID $($proc.Id)). stop.ps1 -RunDir $runDir 로 내린다" }

$run.windowId = $window.id
$run | ConvertTo-Json | Set-Content (Join-Path $runDir 'run.json') -Encoding UTF8
Write-Output "PID=$($proc.Id) WINDOW=$($window.id) TITLE=$($window.title) SIZE=$($window.width)x$($window.height)"
Write-Output "RUN_DIR=$runDir"
