<#
.SYNOPSIS
  Cut a Larrier release: bump the version, write its changelog section,
  commit and tag it, build and check the Windows program, and (with
  -Publish) push and publish it on GitHub. Works without GitHub Actions.

.DESCRIPTION
  Versions follow Semantic Versioning (https://semver.org). The version
  lives in Cargo.toml's [workspace.package]; the changelog comes from the
  Conventional Commit subjects since the last release tag:
    feat → New, style → Look and feel, perf → Faster, fix → Fixed,
    a "!" after the type or "BREAKING CHANGE" → Breaking.

.EXAMPLE
  ./scripts/release.ps1 -Notes                  # the next release's notes, nothing changed
  ./scripts/release.ps1 -Bump patch             # 0.1.0 → 0.1.1, built into dist/
  ./scripts/release.ps1 -Bump minor -Publish    # … pushed and published
  ./scripts/release.ps1 -Bump minor -Pre beta   # 0.2.0-beta.1, a preview
  ./scripts/release.ps1 -Pre beta -Publish      # 0.2.0-beta.1 → 0.2.0-beta.2
  ./scripts/release.ps1 -Version 1.0.0 -Publish
#>
param(
    [ValidateSet('patch', 'minor', 'major')] [string] $Bump,
    [string] $Version,
    # Pre-release label (beta, rc, …): the release is a preview.
    [string] $Pre,
    [switch] $Publish,
    # Print the next release's notes (or -For an existing tag's) and stop.
    [switch] $Notes,
    [string] $For,
    # Commit and tag without building (the CI workflow builds instead).
    [switch] $SkipBuild,
    # Extra lines for the release commit's message (e.g. a co-author).
    [string[]] $Trailer = @()
)
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot)

function Fail([string] $msg) { Write-Host "release: $msg" -ForegroundColor Red; exit 1 }
function Invoke-Git { $out = & git.exe @args 2>&1; if ($LASTEXITCODE -ne 0) { Fail "git $($args -join ' '): $out" }; $out }

# ---------------------------------------------------------------- versions

function Parse-Semver([string] $v) {
    if ($v -notmatch '^v?(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?(?:\+.*)?$') { return $null }
    [pscustomobject]@{ Major = [int]$Matches[1]; Minor = [int]$Matches[2]; Patch = [int]$Matches[3]; Pre = $Matches[4] }
}

# -1, 0 or 1 as a is older, the same as or newer than b (a release comes
# after its previews).
function Compare-Semver([string] $a, [string] $b) {
    $x, $y = (Parse-Semver $a), (Parse-Semver $b)
    foreach ($f in 'Major', 'Minor', 'Patch') { if ($x.$f -ne $y.$f) { return [math]::Sign($x.$f - $y.$f) } }
    if (-not $x.Pre -and -not $y.Pre) { return 0 }
    if (-not $x.Pre) { return 1 }
    if (-not $y.Pre) { return -1 }
    $p, $q = $x.Pre.Split('.'), $y.Pre.Split('.')
    for ($i = 0; $i -lt [math]::Min($p.Count, $q.Count); $i++) {
        $pn, $qn = ($p[$i] -match '^\d+$'), ($q[$i] -match '^\d+$')
        $c = if ($pn -and $qn) { [math]::Sign([long]$p[$i] - [long]$q[$i]) } elseif ($pn) { -1 } elseif ($qn) { 1 } else { [math]::Sign([string]::CompareOrdinal($p[$i], $q[$i])) }
        if ($c -ne 0) { return $c }
    }
    [math]::Sign($p.Count - $q.Count)
}

function Current-Version {
    $toml = Get-Content Cargo.toml -Raw
    if ($toml -notmatch '(?ms)^\[workspace\.package\].*?^version\s*=\s*"([^"]+)"') { Fail 'no version in [workspace.package] of Cargo.toml' }
    $Matches[1]
}

function Last-Tag {
    $t = & git describe --tags --abbrev=0 --match 'v[0-9]*' 2>$null
    if ($LASTEXITCODE -eq 0) { $t.Trim() } else { $null }
}

function Next-Version([string] $current) {
    if ($Version) {
        if (-not (Parse-Semver $Version)) { Fail "'$Version' isn't a semantic version (1.2.3 or 1.2.3-beta.1)" }
        return $Version.TrimStart('v')
    }
    $c = Parse-Semver $current
    $core = "$($c.Major).$($c.Minor).$($c.Patch)"
    # A preview's release keeps its number: 0.2.0-beta.3 → patch → 0.2.0.
    switch ($Bump) {
        'major' { if (-not ($c.Pre -and $c.Minor -eq 0 -and $c.Patch -eq 0)) { $core = "$($c.Major + 1).0.0" } }
        'minor' { if (-not ($c.Pre -and $c.Patch -eq 0)) { $core = "$($c.Major).$($c.Minor + 1).0" } }
        'patch' { if (-not $c.Pre) { $core = "$($c.Major).$($c.Minor).$($c.Patch + 1)" } }
        default { if (-not $Pre) { Fail 'say what to release: -Bump patch|minor|major, -Version x.y.z, or -Pre label' } }
    }
    if (-not $Pre) { return $core }
    # The next preview number for this version and label.
    $n = 1
    $tags = & git tag --list "v$core-$Pre.*"
    foreach ($t in $tags) { if ($t -match "\.(\d+)$") { $n = [math]::Max($n, [int]$Matches[1] + 1) } }
    if ($c.Pre -match "^$([regex]::Escape($Pre))\.(\d+)$" -and "$($c.Major).$($c.Minor).$($c.Patch)" -eq $core) { $n = [math]::Max($n, [int]$Matches[1] + 1) }
    "$core-$Pre.$n"
}

# ---------------------------------------------------------------- notes

function Release-Notes([string] $since, [string] $until = 'HEAD') {
    $range = if ($since) { "$since..$until" } else { $until }
    $log = & git log $range --no-merges --format='%s%x1f%b%x1e'
    $groups = [ordered]@{ 'Breaking' = @(); 'New' = @(); 'Look and feel' = @(); 'Faster' = @(); 'Fixed' = @() }
    foreach ($entry in ($log -join "`n") -split [char]0x1e) {
        $parts = $entry.Trim() -split [char]0x1f, 2
        $subject = $parts[0].Trim()
        if ($subject -notmatch '^(\w+)(\([^)]*\))?(!)?:\s*(.+)$') { continue }
        $type, $bang, $text = $Matches[1], $Matches[3], $Matches[4]
        if ($type -eq 'chore' -and $subject -match '^chore\(release\)') { continue }
        $text = $text.Substring(0, 1).ToUpper() + $text.Substring(1)
        if ($bang -or ($parts.Count -gt 1 -and $parts[1] -match 'BREAKING CHANGE')) { $groups['Breaking'] += $text }
        switch ($type) {
            'feat' { $groups['New'] += $text }
            'style' { $groups['Look and feel'] += $text }
            'perf' { $groups['Faster'] += $text }
            'fix' { $groups['Fixed'] += $text }
        }
    }
    $out = @()
    foreach ($k in $groups.Keys) {
        if ($groups[$k].Count -eq 0) { continue }
        $out += "### $k", ''
        # Oldest first reads like the story of the release.
        $items = @($groups[$k]); [array]::Reverse($items)
        $out += @($items | ForEach-Object { "- $_" })
        $out += ''
    }
    if ($out.Count -eq 0) { $out = @('Maintenance release.', '') }
    ($out -join "`n").TrimEnd()
}

$ChangelogHeader = @'
# Changelog

Every Larrier release and what changed in it. Versions follow
[Semantic Versioning](https://semver.org): the major number for breaking
changes, the minor for new features, the patch for fixes. Previews carry a
suffix such as `-beta.2`.
'@

function Write-Changelog([string] $ver, [string] $body) {
    $date = Get-Date -Format 'yyyy-MM-dd'
    $section = "## $ver — $date`n`n$body`n"
    if (Test-Path CHANGELOG.md) {
        $text = (Get-Content CHANGELOG.md -Raw) -replace "`r`n", "`n"
        $i = $text.IndexOf("`n## ")
        $text = if ($i -ge 0) { $text.Substring(0, $i + 1) + "`n" + $section + $text.Substring($i + 1) } else { $text.TrimEnd() + "`n`n" + $section }
    } else {
        $text = $ChangelogHeader + "`n" + $section
    }
    $text = ($text -replace "`n{3,}", "`n`n").TrimEnd() + "`n"
    [IO.File]::WriteAllText((Join-Path (Get-Location) 'CHANGELOG.md'), $text)
}

# ---------------------------------------------------------------- run

$current = Current-Version
$last = Last-Tag
if ($Notes) {
    if ($For) {
        # The tag before it, if any.
        $prev = & git describe --tags --abbrev=0 --match 'v[0-9]*' "$For^" 2>$null
        Release-Notes $(if ($LASTEXITCODE -eq 0) { $prev.Trim() } else { $null }) $For
    } else {
        Write-Host "Since $(if ($last) { $last } else { 'the beginning' }):`n"
        Release-Notes $last
    }
    exit 0
}
$next = Next-Version $current
$tag = "v$next"
$preview = [bool](Parse-Semver $next).Pre

if ((Invoke-Git status --porcelain --untracked-files=no)) { Fail 'commit or stash your changes first' }
$branch = (Invoke-Git rev-parse --abbrev-ref HEAD).Trim()
if ($branch -ne 'main') { Fail "releases are cut from main (you're on $branch)" }
if (& git tag --list $tag) { Fail "$tag already exists" }
if ($last -and (Compare-Semver $next $last) -le 0) { Fail "$tag doesn't come after the last release ($last)" }

Write-Host "Releasing Larrier $next$(if ($preview) { ' (preview)' }) — was $current, last release $(if ($last) { $last } else { 'none' })" -ForegroundColor Cyan
$body = Release-Notes $last

# Version, lockfile, changelog.
if ($next -ne $current) {
    $toml = Get-Content Cargo.toml -Raw
    $toml = [regex]::Replace($toml, '(?ms)(^\[workspace\.package\].*?^version\s*=\s*")[^"]+(")', "`${1}$next`${2}", 1)
    [IO.File]::WriteAllText((Join-Path (Get-Location) 'Cargo.toml'), $toml)
    & cargo update --workspace --offline 2>&1 | Out-Null
    if ($LASTEXITCODE -ne 0) { Fail 'cargo update --workspace failed' }
}
Write-Changelog $next $body
$msg = @("chore(release): $tag", '', "Larrier $next.") + $(if ($Trailer.Count) { @('') + $Trailer } else { @() })
$msgFile = New-TemporaryFile
[IO.File]::WriteAllText($msgFile, ($msg -join "`n") + "`n")
Invoke-Git add Cargo.toml Cargo.lock CHANGELOG.md | Out-Null
Invoke-Git commit -q -F $msgFile | Out-Null
$notesFile = Join-Path ([IO.Path]::GetTempPath()) "larrier-$tag-notes.md"
[IO.File]::WriteAllText($notesFile, $body + "`n")
Invoke-Git tag -a $tag -F $notesFile | Out-Null
Write-Host "Committed and tagged $tag" -ForegroundColor Green

# Build and check the program.
$dist = Join-Path (Get-Location) 'dist'
$asset = "Larrier-$next-windows-x64.exe"
if (-not $SkipBuild) {
    & cargo build --release --locked -p larrier-app
    if ($LASTEXITCODE -ne 0) { Fail 'the release build failed (the commit and tag are local; remove them with: git tag -d ' + $tag + '; git reset --hard HEAD~1)' }
    $exe = 'target/release/larrier-desktop.exe'
    $reported = (& $exe --version | Out-String).Trim()
    if (-not $reported.StartsWith("Larrier $next ")) { Fail "the built program says '$reported', not Larrier $next" }
    New-Item -ItemType Directory -Force $dist | Out-Null
    Copy-Item $exe (Join-Path $dist $asset) -Force
    $hash = (Get-FileHash (Join-Path $dist $asset) -Algorithm SHA256).Hash.ToLower()
    [IO.File]::WriteAllText((Join-Path $dist "$asset.sha256"), "$hash  $asset")
    Write-Host "Built $reported → dist/$asset" -ForegroundColor Green
}

if (-not $Publish) {
    Write-Host "Not published. To publish: git push origin main $tag; gh release create $tag dist/$asset dist/$asset.sha256 --notes-file `"$notesFile`"$(if ($preview) { ' --prerelease' })"
    exit 0
}
# Push with the GitHub CLI's login when there is one (it also publishes),
# so a broken or missing git credential helper doesn't stop the release.
$auth = @()
& gh auth status *> $null
if ($LASTEXITCODE -eq 0) { $auth = @('-c', 'credential.helper=', '-c', 'credential.helper=!gh auth git-credential') }
Invoke-Git @auth push -q origin HEAD:main | Out-Null
Invoke-Git @auth push -q origin $tag | Out-Null
if (-not $SkipBuild) {
    $ghArgs = @('release', 'create', $tag, (Join-Path $dist $asset), (Join-Path $dist "$asset.sha256"), '--title', "Larrier $next", '--notes-file', $notesFile, '--verify-tag')
    $ghArgs += $(if ($preview) { '--prerelease' } else { '--latest' })
    & gh @ghArgs
    if ($LASTEXITCODE -ne 0) { Fail "gh release create failed; the tag is pushed, so retry: gh $($ghArgs -join ' ')" }
}
Write-Host "Published Larrier $next" -ForegroundColor Green
