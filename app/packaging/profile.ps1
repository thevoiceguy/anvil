# The installer's profile step (windows.iss, docs/MANAGED_INSTALLS.md §2.3):
# puts a company's profile in {app}\profile, writes the single fields the
# command line gave over it, makes icon.ico from the brand's PNG, and tells
# the installer what to name the shortcuts.
#
#   profile.ps1 -App <{app}> -Out <ini> [-Source <dir, .zip or https URL>]
#               [-Embedded <.zip>] [-Server …] [-LoginDomain …] [-SignIn …]
#               [-LockServer …]
#
# -Source (/PROFILE=) goes first, then -Embedded (a branded setup's own);
# with neither, the profile already installed is kept, as a self-update
# keeps it. The ini (UTF-16, as Windows' ini functions read it) holds
# [profile] name, file and icon, or error.

param(
    [Parameter(Mandatory)][string]$App,
    [Parameter(Mandatory)][string]$Out,
    [string]$Source = '',
    [string]$Embedded = '',
    [string]$Server = '',
    [string]$LoginDomain = '',
    [string]$SignIn = '',
    [string]$LockServer = ''
)

$ErrorActionPreference = 'Stop'
$utf8 = New-Object System.Text.UTF8Encoding $false
$dir = Join-Path $App 'profile'

function Write-Result([hashtable]$values) {
    $lines = @('[profile]') + ($values.GetEnumerator() | ForEach-Object { "$($_.Key)=$($_.Value)" })
    [IO.File]::WriteAllText($Out, ($lines -join "`r`n") + "`r`n", [Text.Encoding]::Unicode)
}

function Read-Profile([string]$path) {
    $text = [IO.File]::ReadAllText($path, $utf8).TrimStart([char]0xFEFF)
    $json = $text | ConvertFrom-Json
    if ($json -isnot [pscustomobject]) { throw "$path is not a JSON object" }
    $json
}

# The directory holding profile.json, from a directory, a .zip or an https
# URL of one.
function Open-Source([string]$src, [string]$work) {
    if ($src -match '^https://') {
        [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
        $file = Join-Path $work 'download.zip'
        Invoke-WebRequest -UseBasicParsing -Uri $src -OutFile $file
        $src = $file
    } elseif ($src -match '^[a-zA-Z][a-zA-Z0-9+.-]+://') {
        throw "a profile is fetched over https only: $src"
    }
    if (Test-Path -LiteralPath $src -PathType Container) {
        $root = $src
    } elseif (Test-Path -LiteralPath $src -PathType Leaf) {
        Add-Type -AssemblyName System.IO.Compression.FileSystem
        $root = Join-Path $work 'unzipped'
        [IO.Compression.ZipFile]::ExtractToDirectory($src, $root)
        # profile.json at the top, or in the one folder the zip was made of.
        if (-not (Test-Path -LiteralPath (Join-Path $root 'profile.json'))) {
            $inner = Get-ChildItem -LiteralPath $root -Directory |
                Where-Object { Test-Path -LiteralPath (Join-Path $_.FullName 'profile.json') } |
                Select-Object -First 1
            if ($inner) { $root = $inner.FullName }
        }
    } else {
        throw "no profile at $src"
    }
    $json = Join-Path $root 'profile.json'
    if (-not (Test-Path -LiteralPath $json)) { throw "no profile.json in $src" }
    Read-Profile $json | Out-Null
    $root
}

# PNG images in an ICO, at 16, 32, 48 and 256 pixels; the PNG as it is when
# it cannot be scaled here.
function ConvertTo-Ico([string]$png, [string]$ico) {
    $bytes = [IO.File]::ReadAllBytes($png)
    $signature = [byte[]](0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A)
    if ($bytes.Length -lt 24) { throw "the brand's icon is not a PNG: $png" }
    for ($i = 0; $i -lt 8; $i++) {
        if ($bytes[$i] -ne $signature[$i]) { throw "the brand's icon is not a PNG: $png" }
    }
    $images = New-Object System.Collections.Generic.List[object]
    try {
        Add-Type -AssemblyName System.Drawing
        $stream = New-Object IO.MemoryStream (, $bytes)
        $source = [Drawing.Image]::FromStream($stream)
        foreach ($size in 16, 32, 48, 256) {
            $bitmap = New-Object Drawing.Bitmap $size, $size
            $g = [Drawing.Graphics]::FromImage($bitmap)
            $g.InterpolationMode = [Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
            $g.DrawImage($source, 0, 0, $size, $size)
            $g.Dispose()
            $out = New-Object IO.MemoryStream
            $bitmap.Save($out, [Drawing.Imaging.ImageFormat]::Png)
            $bitmap.Dispose()
            $images.Add([pscustomobject]@{ W = $size; H = $size; Data = $out.ToArray() })
        }
        $source.Dispose()
    } catch {
        $images.Clear()
        $w = ([int]$bytes[16] -shl 24) -bor ([int]$bytes[17] -shl 16) -bor ([int]$bytes[18] -shl 8) -bor $bytes[19]
        $h = ([int]$bytes[20] -shl 24) -bor ([int]$bytes[21] -shl 16) -bor ([int]$bytes[22] -shl 8) -bor $bytes[23]
        $images.Add([pscustomobject]@{ W = $w; H = $h; Data = $bytes })
    }
    $file = New-Object IO.MemoryStream
    $writer = New-Object IO.BinaryWriter $file
    $writer.Write([uint16]0)
    $writer.Write([uint16]1)
    $writer.Write([uint16]$images.Count)
    $offset = 6 + 16 * $images.Count
    foreach ($image in $images) {
        # 0 is 256 (or more) in an ICO's directory.
        $writer.Write([byte]$(if ($image.W -ge 256) { 0 } else { $image.W }))
        $writer.Write([byte]$(if ($image.H -ge 256) { 0 } else { $image.H }))
        $writer.Write([byte]0)
        $writer.Write([byte]0)
        $writer.Write([uint16]1)
        $writer.Write([uint16]32)
        $writer.Write([uint32]$image.Data.Length)
        $writer.Write([uint32]$offset)
        $offset += $image.Data.Length
    }
    foreach ($image in $images) { $writer.Write([byte[]]$image.Data) }
    $writer.Flush()
    [IO.File]::WriteAllBytes($ico, $file.ToArray())
}

$work = Join-Path ([IO.Path]::GetTempPath()) ('anvil-profile-' + [guid]::NewGuid())
New-Item -ItemType Directory -Path $work | Out-Null
try {
    $from = if ($Source) { $Source } elseif ($Embedded) { $Embedded } else { '' }
    if ($from) {
        $root = Open-Source $from $work
        # The shortcuts the installer made before are kept on record, so it
        # can take away the ones the new profile renames.
        $made = Join-Path $dir 'shortcuts.json'
        $kept = if (Test-Path -LiteralPath $made) { [IO.File]::ReadAllBytes($made) } else { $null }
        if (Test-Path -LiteralPath $dir) { Remove-Item -LiteralPath $dir -Recurse -Force }
        New-Item -ItemType Directory -Path $App -Force | Out-Null
        Copy-Item -LiteralPath $root -Destination $dir -Recurse
        Remove-Item -LiteralPath (Join-Path $dir 'icon.ico') -ErrorAction SilentlyContinue
        if ($kept) { [IO.File]::WriteAllBytes($made, $kept) }
    }

    $fields = [ordered]@{}
    if ($Server) { $fields.server = $Server.Trim() }
    if ($LoginDomain) { $fields.login_domain = $LoginDomain.Trim().TrimStart('@') }
    if ($SignIn) {
        $mode = $SignIn.Trim().ToLowerInvariant()
        if ($mode -notin 'auto', 'sso', 'password') { throw "/SIGNIN= is auto, sso or password, not $SignIn" }
        $fields.sign_in = $mode
    }
    if ($LockServer) {
        switch -regex ($LockServer.Trim()) {
            '^(1|true|yes)$' { $fields.lock_server = $true }
            '^(0|false|no)$' { $fields.lock_server = $false }
            default { throw "/LOCKSERVER= is 1 or 0, not $LockServer" }
        }
    }
    $path = Join-Path $dir 'profile.json'
    if ($fields.Count) {
        New-Item -ItemType Directory -Path $dir -Force | Out-Null
        $json = if (Test-Path -LiteralPath $path) { Read-Profile $path } else { [pscustomobject]@{ profile_version = 1 } }
        foreach ($key in $fields.Keys) {
            $json | Add-Member -NotePropertyName $key -NotePropertyValue $fields[$key] -Force
        }
        [IO.File]::WriteAllText($path, ($json | ConvertTo-Json -Depth 20), $utf8)
    }

    $name = 'Anvil'
    $icon = ''
    if (Test-Path -LiteralPath $path) {
        $brand = (Read-Profile $path).brand
        if ($brand -and "$($brand.app_name)".Trim()) { $name = "$($brand.app_name)".Trim() }
        $png = "$($brand.icon)"
        # A file beside profile.json, never one elsewhere (as the app reads it).
        if ($brand -and $png -and [IO.Path]::GetFileName($png) -eq $png) {
            $file = Join-Path $dir $png
            if (Test-Path -LiteralPath $file) {
                $icon = Join-Path $dir 'icon.ico'
                ConvertTo-Ico $file $icon
            }
        }
    }
    # A name Windows takes for a file; Anvil's if nothing is left of it.
    $file = ($name -replace '[\\/:*?"<>|\x00-\x1f]', '').Trim().TrimEnd('.')
    if (-not $file) { $file = 'Anvil' }
    Write-Result @{ name = $name; file = $file; icon = $icon }
} catch {
    Write-Result @{ error = ($_.Exception.Message -replace '[\r\n]+', ' ') }
    exit 1
} finally {
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
}
