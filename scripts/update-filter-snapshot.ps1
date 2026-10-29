# Refreshes the filter data shipped with Jello in src-tauri/resources/content-blocker:
# the redistributable default filter lists and the uBlock Origin files the built-in
# blocker reads its scriptlets and redirect resources from. Files are stored exactly
# as published. Run before a release:
#   powershell -ExecutionPolicy Bypass -File scripts/update-filter-snapshot.ps1
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$root = Split-Path -Parent $PSScriptRoot
$dest = Join-Path $root 'src-tauri\resources\content-blocker'
$headers = @{ 'User-Agent' = 'Jello filter snapshot' }

# Peter Lowe's list is not included: it is published without a redistribution
# license, so Jello downloads it at runtime only.
$lists = [ordered]@{
  'ublock-filters'     = 'https://ublockorigin.github.io/uAssets/filters/filters.txt'
  'ublock-badware'     = 'https://ublockorigin.github.io/uAssets/filters/badware.txt'
  'ublock-privacy'     = 'https://ublockorigin.github.io/uAssets/filters/privacy.txt'
  'ublock-quick-fixes' = 'https://ublockorigin.github.io/uAssets/filters/quick-fixes.txt'
  'ublock-unbreak'     = 'https://ublockorigin.github.io/uAssets/filters/unbreak.txt'
  'easylist'           = 'https://easylist.to/easylist/easylist.txt'
  'easyprivacy'        = 'https://easylist.to/easylist/easyprivacy.txt'
}

$work = Join-Path ([IO.Path]::GetTempPath()) ('jello-snapshot-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force $work | Out-Null
try {
  $staging = Join-Path $work 'content-blocker'
  New-Item -ItemType Directory -Force (Join-Path $staging 'lists') | Out-Null

  foreach ($id in $lists.Keys) {
    $out = Join-Path $staging "lists\$id.txt"
    Invoke-WebRequest -Uri $lists[$id] -Headers $headers -OutFile $out
    $head = Get-Content $out -TotalCount 1
    if ($head -match '^\s*<') { throw "$id did not download as a filter list" }
    Write-Host "list $id"
  }

  $release = Invoke-RestMethod -Uri 'https://api.github.com/repos/gorhill/uBlock/releases/latest' -Headers $headers
  $asset = $release.assets | Where-Object { $_.name -like '*.chromium.zip' } | Select-Object -First 1
  if (-not $asset) { throw "uBlock Origin $($release.tag_name) has no Chromium zip" }
  $zip = Join-Path $work 'ubo.zip'
  Invoke-WebRequest -Uri $asset.browser_download_url -Headers $headers -OutFile $zip
  $unzipped = Join-Path $work 'ubo'
  Expand-Archive -Path $zip -DestinationPath $unzipped
  $src = Get-ChildItem $unzipped -Directory | Select-Object -First 1

  # The scriptlet modules and everything they import, plus the redirect resources.
  $ubo = Join-Path $staging 'ublock-origin'
  $pending = [Collections.Generic.Queue[string]]::new()
  $pending.Enqueue('js/resources/scriptlets.js')
  $pending.Enqueue('js/redirect-resources.js')
  $copied = @{}
  while ($pending.Count) {
    $rel = $pending.Dequeue()
    if ($copied.ContainsKey($rel)) { continue }
    $copied[$rel] = $true
    $from = Join-Path $src.FullName $rel
    $to = Join-Path $ubo $rel
    New-Item -ItemType Directory -Force (Split-Path $to) | Out-Null
    Copy-Item $from $to
    $text = Get-Content $from -Raw
    foreach ($m in [regex]::Matches($text, '(?:import|export)\s[^;]*?from\s*[''"](\.[^''"]+)[''"]|import\s+[''"](\.[^''"]+)[''"]')) {
      $spec = if ($m.Groups[1].Success) { $m.Groups[1].Value } else { $m.Groups[2].Value }
      $dir = [IO.Path]::GetDirectoryName($rel.Replace('/', '\'))
      $full = [IO.Path]::GetFullPath([IO.Path]::Combine('C:\x', $dir, $spec.Replace('/', '\')))
      $pending.Enqueue($full.Substring(5).Replace('\', '/'))
    }
  }
  Copy-Item (Join-Path $src.FullName 'web_accessible_resources') (Join-Path $ubo 'web_accessible_resources') -Recurse

  Invoke-WebRequest -Uri 'https://www.gnu.org/licenses/gpl-3.0.txt' -Headers $headers -OutFile (Join-Path $staging 'LICENSE-GPL-3.0.txt')

  $snapshot = [ordered]@{
    taken         = (Get-Date).ToUniversalTime().ToString('yyyy-MM-dd')
    ublockOrigin  = $release.tag_name
    lists         = $lists
  }
  [IO.File]::WriteAllText((Join-Path $staging 'snapshot.json'), ($snapshot | ConvertTo-Json))
  Copy-Item (Join-Path $dest 'README.md') (Join-Path $staging 'README.md') -ErrorAction SilentlyContinue

  if (Test-Path $dest) { Remove-Item -Recurse -Force $dest }
  Move-Item $staging $dest
  Write-Host "snapshot: $($lists.Count) lists, uBlock Origin $($release.tag_name), $($copied.Count) scriptlet modules"
} finally {
  Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}
