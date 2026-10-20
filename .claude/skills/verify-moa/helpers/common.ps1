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

# Karina CLI 를 불러 JSON 을 객체로 돌려준다. 출력은 파이프로 읽으면서 기다린다(교착 방지).
# JSON 으로 읽는 것은 stdout 뿐이다 — stderr 의 경고·안내 줄이 섞이면 파싱이 깨진다.
# 실패해도 JSON(ok=false)을 내는 명령이 있어, 0 아닌 종료 코드는 ok=false 가 아닐 때만 실패로 본다
function Invoke-Karina {
    param([Parameter(Mandatory)][string[]]$Arguments)
    $cli = Get-KarinaCli
    $errLines = [System.Collections.Generic.List[string]]::new()
    $text = (& $cli @Arguments 2>&1 | ForEach-Object {
            if ($_ -is [System.Management.Automation.ErrorRecord]) { $errLines.Add("$_") } else { $_ }
        } | Out-String)
    $exit = $LASTEXITCODE
    $stderr = $errLines -join "`n"
    try { $res = $text | ConvertFrom-Json } catch { throw "Karina CLI 출력이 JSON 이 아니다 (exit $exit): $text$(if ($stderr) { " / stderr: $stderr" })" }
    if ($exit -ne 0 -and $res.ok -ne $false) { throw "Karina CLI 가 exit $exit 로 끝났다: $text$(if ($stderr) { " / stderr: $stderr" })" }
    return $res
}

# 8.3 짧은 경로(C:\Users\LONGNA~1\...)를 긴 경로로 편다 — %TEMP% 가 짧은 경로인 프로필이 있고,
# Process.Path 는 늘 긴 경로라 그대로 비교하면 우리 사본을 남의 것으로 본다
Add-Type -Namespace VerifyMoa -Name PathApi -MemberDefinition @'
[DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
public static extern uint GetLongPathNameW(string shortPath, System.Text.StringBuilder longPath, uint bufferLength);
'@
function Get-LongPath {
    param([Parameter(Mandatory)][string]$Path)
    $buf = [System.Text.StringBuilder]::new(32768)
    $len = [VerifyMoa.PathApi]::GetLongPathNameW($Path, $buf, [uint32]$buf.Capacity)
    if ($len -eq 0 -or $len -gt $buf.Capacity) { return $Path }   # 없는 경로 등 — 받은 그대로 둔다
    return $buf.ToString()
}

# 두 경로가 같은 파일을 가리키는가 — 짧은 경로·대소문자 차이를 무시한다
function Test-SamePath {
    param([string]$A, [string]$B)
    if (-not $A -or -not $B) { return $false }
    return [string]::Equals((Get-LongPath $A), (Get-LongPath $B), [System.StringComparison]::OrdinalIgnoreCase)
}

# 이 로그온 세션의 다른 MOA 를 찾는다 — 중복 실행 방지 뮤텍스가 Local\ 이라 다른 세션(다른 사용자·RDP)의 MOA 는 충돌하지 않는다
function Get-OtherMoa {
    param([int]$ExceptPid = 0)
    $session = (Get-Process -Id $PID).SessionId
    return Get-Process moa -ErrorAction SilentlyContinue | Where-Object { $_.SessionId -eq $session -and $_.Id -ne $ExceptPid }
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
