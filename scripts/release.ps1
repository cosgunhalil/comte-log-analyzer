<#
.SYNOPSIS
    Prepares a release: next version, CHANGELOG section, commit and tag.

.DESCRIPTION
    1. Finds the latest vX.Y.Z tag and picks the next patch version
       (or the one given with -Version).
    2. Writes a CHANGELOG.md section from the Conventional Commits since that
       tag (feat -> Added, fix -> Fixed, perf -> Performance, the rest -> Other).
    3. Sets that version in [workspace.package] of Cargo.toml and in Cargo.lock.
    4. Commits "chore(release): vX.Y.Z" and creates the tag vX.Y.Z on it.

    Nothing leaves your machine unless you pass -Push. Pushing the tag starts
    .github/workflows/release.yml, which builds and publishes the exe and zip.

.EXAMPLE
    ./scripts/release.ps1 -DryRun     # show the version and changelog, change nothing
.EXAMPLE
    ./scripts/release.ps1             # next patch: commit and tag locally
.EXAMPLE
    ./scripts/release.ps1 -Version 0.2.0 -Push
#>
[CmdletBinding()]
param(
    # The version to release (X.Y.Z). Default: the latest tag's next patch.
    [string]$Version,
    # Show what would happen; change nothing.
    [switch]$DryRun,
    # Also push the commit and the tag (which publishes the release).
    [switch]$Push
)

$ErrorActionPreference = 'Stop'
$repoUrl = 'https://github.com/cosgunhalil/comte-log-analyzer'
$root = (& git.exe rev-parse --show-toplevel).Trim()
Set-Location $root
$utf8 = New-Object System.Text.UTF8Encoding $false

function Fail([string]$message) {
    Write-Host "release: $message" -ForegroundColor Red
    exit 1
}

# Runs git; stops on failure. (stderr is not redirected: Windows PowerShell 5.1
# turns redirected stderr of native commands into errors, and git writes
# progress there even when it succeeds.)
function Invoke-Git([string[]]$arguments) {
    $output = & git.exe @arguments
    if ($LASTEXITCODE -ne 0) { Fail "git $($arguments -join ' ') failed." }
    return $output
}

# --- checks -------------------------------------------------------------------
if (-not $DryRun) {
    if (Invoke-Git @('status', '--porcelain')) { Fail 'the working tree has uncommitted changes; commit or stash them first.' }
    $branch = (Invoke-Git @('rev-parse', '--abbrev-ref', 'HEAD')).Trim()
    if ($branch -ne 'master') { Fail "releases are cut from master (you are on $branch)." }
}

# --- version ------------------------------------------------------------------
$tags = @(Invoke-Git @('tag', '--list', 'v*')) | Where-Object { $_ -match '^v\d+\.\d+\.\d+$' }
$latest = $tags | Sort-Object { [version]($_.Substring(1)) } | Select-Object -Last 1
if (-not $Version) {
    if (-not $latest) { Fail 'no vX.Y.Z tag yet; pass -Version.' }
    $v = [version]($latest.Substring(1))
    $Version = '{0}.{1}.{2}' -f $v.Major, $v.Minor, ($v.Build + 1)
}
if ($Version -notmatch '^\d+\.\d+\.\d+$') { Fail "'$Version' is not a version like 1.2.3." }
$tag = "v$Version"
if ($tags -contains $tag) { Fail "$tag already exists. Releases never reuse a tag; pick another version." }
if ($latest -and ([version]$Version -le [version]($latest.Substring(1)))) {
    Fail "$tag is not newer than the latest tag $latest."
}

# --- changelog section ----------------------------------------------------------
$range = if ($latest) { "$latest..HEAD" } else { 'HEAD' }
$subjects = @(Invoke-Git @('log', $range, '--no-merges', '--format=%s'))
$groups = [ordered]@{ 'Added' = @(); 'Fixed' = @(); 'Performance' = @(); 'Other' = @() }
foreach ($subject in $subjects) {
    if ($subject -match '^chore\(release\)') { continue }
    if ($subject -match '^(?<type>[a-z]+)(\((?<scope>[^)]+)\))?(?<bang>!)?: (?<text>.+)$') {
        $line = '- '
        if ($Matches['scope']) { $line += "*($($Matches['scope']))* " }
        elseif ($Matches['type'] -notin @('feat', 'fix', 'perf')) { $line += "*($($Matches['type']))* " }
        if ($Matches['bang']) { $line += '**breaking:** ' }
        $line += $Matches['text']
        switch ($Matches['type']) {
            'feat' { $groups['Added'] += $line }
            'fix' { $groups['Fixed'] += $line }
            'perf' { $groups['Performance'] += $line }
            default { $groups['Other'] += $line }
        }
    }
    else {
        $groups['Other'] += "- $subject"
    }
}
$entries = 0
foreach ($g in $groups.Values) { $entries += $g.Count }
if ($entries -eq 0) { Fail "no commits since ${latest}: nothing to release." }

$date = Get-Date -Format 'yyyy-MM-dd'
$section = @("## [$Version]($repoUrl/releases/tag/$tag) - $date", '')
foreach ($name in $groups.Keys) {
    if ($groups[$name].Count -eq 0) { continue }
    $section += "### $name"
    $section += ''
    # git log lists newest first; the changelog reads oldest first.
    $items = $groups[$name]
    [array]::Reverse($items)
    $section += $items
    $section += ''
}

Write-Host "Release $tag (previous: $(if ($latest) { $latest } else { 'none' }))" -ForegroundColor Cyan
Write-Host ''
$section | ForEach-Object { Write-Host "  $_" }
if ($DryRun) {
    Write-Host 'Dry run: nothing changed.' -ForegroundColor Yellow
    exit 0
}

# --- write files ----------------------------------------------------------------
$changelogPath = Join-Path $root 'CHANGELOG.md'
$changelog = [System.IO.File]::ReadAllText($changelogPath)
$nl = if ($changelog.Contains("`r`n")) { "`r`n" } else { "`n" }
$marker = '## [Unreleased]'
$at = $changelog.IndexOf($marker)
if ($at -lt 0) { Fail "CHANGELOG.md has no '$marker' heading." }
$insertAt = $at + $marker.Length
$changelog = $changelog.Substring(0, $insertAt) + $nl + $nl + ($section -join $nl).TrimEnd() + $changelog.Substring($insertAt)
[System.IO.File]::WriteAllText($changelogPath, $changelog, $utf8)

$cargoPath = Join-Path $root 'Cargo.toml'
$cargo = [System.IO.File]::ReadAllText($cargoPath)
$pattern = '(?ms)(^\[workspace\.package\]\s*$.*?^version\s*=\s*")[^"]+(")'
if ($cargo -notmatch $pattern) { Fail 'no version in [workspace.package] of Cargo.toml.' }
$cargo = [regex]::Replace($cargo, $pattern, "`${1}$Version`${2}", 1)
[System.IO.File]::WriteAllText($cargoPath, $cargo, $utf8)

& cargo update --workspace --offline --quiet
if ($LASTEXITCODE -ne 0) { Fail 'cargo update --workspace failed.' }

# --- commit and tag ---------------------------------------------------------------
Invoke-Git @('add', 'Cargo.toml', 'Cargo.lock', 'CHANGELOG.md') | Out-Null
Invoke-Git @('commit', '--quiet', '-m', "chore(release): $tag") | Out-Null
Invoke-Git @('tag', '-a', $tag, '-m', "Comte Log Analyzer $tag") | Out-Null
Write-Host ''
Write-Host "Committed and tagged $tag." -ForegroundColor Green

if ($Push) {
    Invoke-Git @('push', 'origin', 'master') | Out-Null
    Invoke-Git @('push', 'origin', $tag) | Out-Null
    Write-Host "Pushed. The release builds at $repoUrl/actions" -ForegroundColor Green
}
else {
    Write-Host 'Publish it by pushing both:'
    Write-Host '  git push origin master'
    Write-Host "  git push origin $tag"
}
