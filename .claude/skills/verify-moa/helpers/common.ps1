# verify-moa 헬퍼가 함께 쓰는 함수 — 다른 헬퍼가 dot-source 한다. 직접 실행하지 않는다.

$ErrorActionPreference = 'Stop'

# Karina CLI 를 고른다 — karina-computer-use 의 Preconditions 순서(환경변수 → PATH → 설치본)
function Get-KarinaCli {
    if ($env:KARINA_CLI_COMMAND -and (Test-Path $env:KARINA_CLI_COMMAND)) { return $env:KARINA_CLI_COMMAND }
    foreach ($name in 'karina-cli', 'karina-dev-cli') {
        $cmd = Get-Command $name -ErrorAction SilentlyContinue
        if ($cmd) { return $cmd.Source }
    }
    $installed = Join-Path $env:LOCALAPPDATA 'Programs\Karina\karina-cli.exe'
    if (Test-Path $installed) { return $installed }
    throw 'Karina CLI 를 찾지 못했다 — KARINA_CLI_COMMAND·PATH·설치본 어디에도 없다'
}

# Karina CLI 를 불러 JSON 을 객체로 돌려준다. 출력은 파이프로 읽으면서 기다린다(교착 방지)
function Invoke-Karina {
    param([Parameter(Mandatory)][string[]]$Arguments)
    $cli = Get-KarinaCli
    $text = (& $cli @Arguments 2>&1 | Out-String)
    try { return $text | ConvertFrom-Json } catch { throw "Karina CLI 출력이 JSON 이 아니다: $text" }
}

# 실행 폴더의 run.json 을 읽는다
function Read-Run {
    param([Parameter(Mandatory)][string]$RunDir)
    $path = Join-Path $RunDir 'run.json'
    if (-not (Test-Path $path)) { throw "run.json 이 없다: $path — launch.ps1 이 만든 실행 폴더인가" }
    return Get-Content $path -Raw -Encoding UTF8 | ConvertFrom-Json
}

# 이 PID 의 MOA 주 창을 찾는다 — 제목 없는 16x16 창(트레이 메시지 창)은 건너뛴다
function Find-MainWindow {
    param([Parameter(Mandatory)][int]$ProcessId)
    $res = Invoke-Karina @('computer', 'list-windows', '--app', "pid:$ProcessId", '--json')
    if (-not $res.ok) { return $null }
    return $res.windows | Where-Object { $_.title -and $_.width -gt 200 -and -not $_.isMinimized } | Select-Object -First 1
}
