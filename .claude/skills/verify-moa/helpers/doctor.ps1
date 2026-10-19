# 「이 MOA 인스턴스를 조작해도 되는가」 읽기 전용 점검. 아무것도 바꾸지 않는다.
#
# 사용: pwsh -File doctor.ps1 -RunDir <실행 폴더>
# 각 줄 OK/FAIL 로 찍고, FAIL 이 하나라도 있으면 exit 1.
param([Parameter(Mandatory)][string]$RunDir)
. (Join-Path $PSScriptRoot 'common.ps1')
$ErrorActionPreference = 'Continue'

$fails = 0
function Report([bool]$ok, [string]$what) {
    if ($ok) { Write-Output "OK   $what" } else { Write-Output "FAIL $what"; $script:fails++ }
}

$run = Read-Run -RunDir $RunDir

# 1. 조작 도구 — 스킬을 만든 PC·시점과 돌리는 PC·시점이 다를 수 있어 매번 본다
$cli = $null
try { $cli = Get-KarinaCli } catch { }
Report ([bool]$cli) "Karina CLI: $cli"
if ($cli) {
    & $cli skills get karina-computer-use | Out-Null
    Report ($LASTEXITCODE -eq 0) "karina-cli skills get karina-computer-use exit $LASTEXITCODE"
}

# 2. 프로세스가 우리가 띄운 사본인가
$proc = Get-Process -Id $run.pid -ErrorAction SilentlyContinue
Report ([bool]$proc) "PID $($run.pid) 살아 있음"
if ($proc) { Report ($proc.Path -eq $run.exe) "PID $($run.pid) 경로 = $($proc.Path) (기대 $($run.exe))" }

# 3. 다른 MOA 가 없는가 — 있으면 중복 실행 방지가 입력·세션 저장을 섞는다
$others = Get-Process moa -ErrorAction SilentlyContinue | Where-Object { $_.Id -ne $run.pid }
Report (-not $others) "다른 MOA 없음$(if ($others) { ' — ' + (($others | ForEach-Object { "PID $($_.Id) $($_.Path)" }) -join '; ') })"

# 4. 기대한 빌드인가 — 실행 중인 사본이 지금 레포 빌드 산출물과 같은가
if (Test-Path $run.exe) {
    $hash = (Get-FileHash $run.exe -Algorithm SHA256).Hash
    Report ($hash -eq $run.exeSha256) "사본 해시가 launch 때와 같음"
    if (Test-Path $run.builtExe) {
        $built = (Get-FileHash $run.builtExe -Algorithm SHA256).Hash
        Report ($built -eq $hash) "사본 = 레포 빌드 산출물 ($($run.builtExe))"
    }
}

# 5. 창이 보이고 화면이 잠기지 않았는가 — 스크린샷 한 장으로 본다
if ($proc -and $cli) {
    $window = Find-MainWindow -ProcessId $run.pid
    Report ([bool]$window) "주 창 보임$(if ($window) { " (id $($window.id), $($window.width)x$($window.height))" })"
    if ($window) {
        # 잠긴 화면·보안 데스크톱이면 커서·입력이 닿지 않는다 — Karina 가 창 상태를 읽을 수 있는지로 본다
        $state = Invoke-Karina @('computer', 'get-app-state', '--app', "pid:$($run.pid)", '--window-id', "$($window.id)", '--no-screenshot', '--json')
        Report ([bool]$state.ok) "Karina 가 창 상태를 읽음$(if (-not $state.ok) { " — $($state.code) $($state.error)" })"
        Report ($window.id -eq $run.windowId) "창 id 가 launch 때와 같음 ($($run.windowId))"
    }
}

if ($fails -gt 0) { Write-Output "DOCTOR: FAIL $fails 건 — 조작하지 않는다"; exit 1 }
Write-Output 'DOCTOR: OK'
