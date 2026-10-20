# 검증 중인 MOA 창의 스크린샷을 증거 폴더에 이름 붙여 남긴다.
#
# 사용: pwsh -File shot.ps1 -RunDir <실행 폴더> -Name <증거 이름> [-NoRaise]
# 결과: evidence\<Name>.png 와 evidence\<Name>.json(창 위치·크기·전경 판정).
# 마지막 줄에 SHOT=<png 경로> 를 찍는다 — 다음 클릭 좌표는 이 그림의 픽셀(창 기준)로 읽는다.
#
# Karina 의 get-app-state 스크린샷(PrintWindow)은 MOA 의 OpenGL 창을 빈 회색으로 잡는다
# (2026-10-19 실측 — --restore-window 를 줘도 같다). 그래서 창을 앞으로 올린 뒤 화면 영역을
# 직접 복사한다. 그 영역에 다른 창이 덮여 있으면 그 창이 찍히므로, 전경 창이 MOA 인지
# 확인하고 아니면 실패한다.
param(
    [Parameter(Mandatory)][string]$RunDir,
    [Parameter(Mandatory)][string]$Name,
    [switch]$NoRaise
)
. (Join-Path $PSScriptRoot 'common.ps1')
Add-Type -AssemblyName System.Drawing
Add-Type -Namespace VerifyMoa -Name Win32 -MemberDefinition '[DllImport("user32.dll")] public static extern System.IntPtr GetForegroundWindow();'

$run = Read-Run -RunDir $RunDir
if (-not $NoRaise) {
    # 창을 최소화·복원해 앞으로 올린다 — 화면이 한 번 깜박인다
    $null = Invoke-Karina @('computer', 'get-app-state', '--app', "pid:$($run.pid)", '--window-id', "$($run.windowId)", '--restore-window', '--no-screenshot', '--json')
    Start-Sleep -Milliseconds 400
}
$window = Find-MainWindow -ProcessId $run.pid
if (-not $window) { throw "PID $($run.pid) 의 주 창을 찾지 못했다" }
$fg = [VerifyMoa.Win32]::GetForegroundWindow().ToInt64()
if ($fg -ne [int64]$window.platform.nativeWindowHandle) {
    throw "전경 창이 MOA 가 아니다(전경 0x$('{0:X}' -f $fg)) — 다른 창이 찍힐 수 있어 캡처하지 않는다. -NoRaise 없이 다시 돌린다"
}

$png = Join-Path $run.evidenceDir "$Name.png"
# 같은 이름을 다시 쓰면 앞 증거(조작 전 캡처 등)가 조용히 사라진다 — 덮어쓰지 않는다
if (Test-Path $png) { throw "증거가 이미 있다: $png — 다른 -Name 을 준다" }
$bmp = [System.Drawing.Bitmap]::new([int]$window.width, [int]$window.height)
$g = [System.Drawing.Graphics]::FromImage($bmp)
try {
    $g.CopyFromScreen([int]$window.x, [int]$window.y, 0, 0, $bmp.Size)
    $bmp.Save($png, [System.Drawing.Imaging.ImageFormat]::Png)
} finally { $g.Dispose(); $bmp.Dispose() }

[ordered]@{
    name = $Name; takenAt = (Get-Date).ToString('o'); pid = $run.pid; windowId = $run.windowId
    x = $window.x; y = $window.y; width = $window.width; height = $window.height; title = $window.title
    foreground = $true
} | ConvertTo-Json | Set-Content (Join-Path $run.evidenceDir "$Name.json") -Encoding UTF8
Write-Output "SHOT=$png SIZE=$($window.width)x$($window.height) TITLE=$($window.title)"
