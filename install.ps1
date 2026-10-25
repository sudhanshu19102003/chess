# Installs chess-tui, the peer-to-peer terminal chess game, on Windows.
#
#   irm https://chess.thesudhanshu.dev/install.ps1 | iex
#
# Picks the build for this CPU (x86_64 or ARM64), checks the release
# signature and the archive's SHA-256, puts chess-tui.exe in
# %LOCALAPPDATA%\Programs\chess and adds that folder to your user PATH.
# No administrator rights. The Windows twin of install.sh.
#
# Settings (environment variables):
#   CHESS_VERSION        a release tag such as v0.1.0 (default: the latest)
#   CHESS_INSTALL_DIR    where to put the binary
#   CHESS_DOWNLOAD_URL   a folder holding the release files, instead of
#                        GitHub Releases (a mirror, or file:///... for testing)
#   CHESS_NO_MODIFY_PATH set to 1 to leave PATH alone
#
# The signature is checked with `ssh-keygen -Y verify` (OpenSSH 8.1 or
# newer), from the OpenSSH Client that Windows 10 and 11 install by default.
# Works in Windows PowerShell 5.1 and PowerShell 7.

$Repo = 'https://github.com/sudhanshu19102003/chess/releases'

# The release signing key. Only SHA256SUMS files signed by it are accepted.
$Signer = 'chess-release'
$Namespace = 'chess-release@sudhanshu19102003'
$PubKey = 'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIKHPBVfEIOY+rZ+qgRHy5c2S3YZ/drAl0Nrg9DWQxeSh'

function Say($msg) { Write-Host "chess: $msg" }

function Get-ChessTarget {
    # A 32-bit or emulated PowerShell reports its own architecture; the
    # machine's is in PROCESSOR_ARCHITEW6432.
    $arch = $env:PROCESSOR_ARCHITEW6432
    if (-not $arch) { $arch = $env:PROCESSOR_ARCHITECTURE }
    switch ($arch) {
        'AMD64' { return 'x86_64-pc-windows-msvc' }
        'ARM64' { return 'aarch64-pc-windows-msvc' }
        default { throw "no build for Windows on $arch; build from source instead:`n  https://github.com/sudhanshu19102003/chess#install" }
    }
}

function Get-File($url, $file) {
    if ($url -match '^file:') {
        Copy-Item -LiteralPath ([Uri]$url).LocalPath -Destination $file
        return
    }
    # Some networks silently drop one of GitHub's download addresses; a
    # timeout and a few tries move on instead of hanging.
    for ($try = 1; ; $try++) {
        try {
            Invoke-WebRequest -Uri $url -OutFile $file -UseBasicParsing -TimeoutSec 60
            return
        } catch {
            if ($try -ge 3) { throw "could not download ${url}: $($_.Exception.Message)" }
            Start-Sleep -Seconds 1
        }
    }
}

function Find-SshKeygen {
    $cmd = Get-Command ssh-keygen.exe -CommandType Application -ErrorAction SilentlyContinue |
        Select-Object -First 1
    if ($cmd) { return $cmd.Source }
    $builtin = Join-Path $env:WINDIR 'System32\OpenSSH\ssh-keygen.exe'
    if (Test-Path -LiteralPath $builtin) { return $builtin }
    throw "need ssh-keygen (OpenSSH Client) to check the signature. Add it in Settings >
  System > Optional features > OpenSSH Client, then run this again."
}

# Runs ssh-keygen with the file as stdin. Piping in PowerShell would re-encode
# the text, so the file goes in through a redirect, byte for byte.
function Test-Signature($keygen, $dir) {
    $q = { param($s) '"' + $s + '"' }
    $argv = @('-Y', 'verify', '-f', (& $q "$dir\allowed_signers"), '-I', $Signer,
        '-n', $Namespace, '-s', (& $q "$dir\SHA256SUMS.sig"))
    $p = Start-Process -FilePath $keygen -ArgumentList $argv -NoNewWindow -Wait -PassThru `
        -RedirectStandardInput "$dir\SHA256SUMS" `
        -RedirectStandardOutput "$dir\verify.out" -RedirectStandardError "$dir\verify.err"
    return $p.ExitCode -eq 0
}

function Add-ToUserPath($dir) {
    # Read and write the raw registry value, so entries like %USERPROFILE%
    # stay unexpanded.
    $key = (Get-Item -Path 'HKCU:\').OpenSubKey('Environment', $true)
    $old = $key.GetValue('Path', '', 'DoNotExpandEnvironmentNames')
    $parts = @($old -split ';' | Where-Object { $_ })
    if ($parts -contains $dir) { $key.Close(); return $false }
    $key.SetValue('Path', (($parts + $dir) -join ';'), 'ExpandString')
    $key.Close()
    # Setting any user variable tells Explorer, and so new terminals, that
    # the environment changed.
    [Environment]::SetEnvironmentVariable('CHESS_INSTALLER', '1', 'User')
    [Environment]::SetEnvironmentVariable('CHESS_INSTALLER', $null, 'User')
    return $true
}

function Install-Chess {
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue' # Invoke-WebRequest is slow with it
    # Windows PowerShell 5.1 may not offer TLS 1.2 by default; GitHub needs it.
    [Net.ServicePointManager]::SecurityProtocol =
        [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

    $target = Get-ChessTarget
    $keygen = Find-SshKeygen

    $version = $env:CHESS_VERSION
    if ($env:CHESS_DOWNLOAD_URL) {
        $base = $env:CHESS_DOWNLOAD_URL.TrimEnd('/')
    } elseif ($version) {
        $base = "$Repo/download/$version"
    } else {
        $base = "$Repo/latest/download"
    }
    $dir = $env:CHESS_INSTALL_DIR
    if (-not $dir) { $dir = Join-Path $env:LOCALAPPDATA 'Programs\chess' }
    $archive = "chess-$target.zip"

    $tmp = Join-Path ([IO.Path]::GetTempPath()) ('chess-' + [Guid]::NewGuid())
    New-Item -ItemType Directory -Path $tmp | Out-Null
    try {
        $shown = $version
        if (-not $shown) { $shown = 'the latest release' }
        Say "downloading $shown for $target"
        Get-File "$base/SHA256SUMS" "$tmp\SHA256SUMS"
        Get-File "$base/SHA256SUMS.sig" "$tmp\SHA256SUMS.sig"

        # Plain ASCII with a Unix line end, as ssh-keygen expects.
        [IO.File]::WriteAllText("$tmp\allowed_signers", "$Signer $PubKey`n")
        if (-not (Test-Signature $keygen $tmp)) {
            throw "SHA256SUMS has no valid signature from the release key; not installing
  (ssh-keygen must be OpenSSH 8.1 or newer: check with ``ssh -V``)"
        }

        $want = $null
        foreach ($line in [IO.File]::ReadAllLines("$tmp\SHA256SUMS")) {
            $f = $line -split '\s+'
            if ($f.Count -ge 2 -and ($f[1] -eq $archive -or $f[1] -eq "*$archive")) { $want = $f[0] }
        }
        if (-not $want) { throw "this release has no $archive" }

        Get-File "$base/$archive" "$tmp\$archive"
        $got = (Get-FileHash -Algorithm SHA256 -LiteralPath "$tmp\$archive").Hash
        if ($got -ne $want) { throw "$archive does not match its signed checksum; not installing" }
        Say 'signature and checksum OK'

        Expand-Archive -LiteralPath "$tmp\$archive" -DestinationPath "$tmp\x"
        $bin = "$tmp\x\chess-$target\chess-tui.exe"
        if (-not (Test-Path -LiteralPath $bin)) { throw "$archive has no chess-tui.exe" }

        New-Item -ItemType Directory -Force -Path $dir | Out-Null
        $dest = Join-Path $dir 'chess-tui.exe'
        # A running exe cannot be overwritten but can be renamed, so move the
        # old one aside and put the new one in its place.
        Copy-Item -LiteralPath $bin -Destination "$dest.new" -Force
        if (Test-Path -LiteralPath "$dest.old") {
            Remove-Item -LiteralPath "$dest.old" -Force -ErrorAction SilentlyContinue
        }
        if (Test-Path -LiteralPath $dest) { Move-Item -LiteralPath $dest -Destination "$dest.old" -Force }
        Move-Item -LiteralPath "$dest.new" -Destination $dest
        Remove-Item -LiteralPath "$dest.old" -Force -ErrorAction SilentlyContinue
        Say "installed $dest"
    } finally {
        Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
    }

    $onPath = ($env:Path -split ';') -contains $dir
    if ($onPath) {
        Say 'run it with: chess-tui'
    } elseif ($env:CHESS_NO_MODIFY_PATH -eq '1') {
        Say "$dir is not on your PATH; run it with: & '$dest'"
    } else {
        Add-ToUserPath $dir | Out-Null
        $env:Path = "$env:Path;$dir"
        Say "added $dir to your PATH"
        Say 'run it with: chess-tui (new terminals find it too)'
    }
}

# Everything runs from here, so a download cut short runs nothing. Under
# `irm | iex` an `exit` would close the user's window, so errors are printed
# instead; run as a file (`powershell -File install.ps1`), they exit with 1.
try {
    Install-Chess
} catch {
    Write-Host "chess: error: $($_.Exception.Message)" -ForegroundColor Red
    if ($PSCommandPath) { exit 1 }
}
