# Builds target\gallery.apk: Rust library (cargo-ndk) + Java service (javac/d8) + manifest (aapt2),
# aligned and signed with your own key (see docs/SIGNING.md).
param([switch]$SkipRust)

$jdk = 'C:\Program Files\Eclipse Adoptium\jdk-17.0.20.101-hotspot'
$sdk = 'E:\Android\sdk'
$env:JAVA_HOME = $jdk
$env:ANDROID_HOME = $sdk
$env:ANDROID_NDK_HOME = 'E:\Android\android-ndk-r27c'
$env:Path = "$jdk\bin;" + [Environment]::GetEnvironmentVariable('Path', 'Machine') + ';' + [Environment]::GetEnvironmentVariable('Path', 'User')

$bt = "$sdk\build-tools\34.0.0"
$jar = "$sdk\platforms\android-34\android.jar"
$root = $PSScriptRoot
$out = "$root\target\apk-build"
$final = "$root\target\gallery.apk"

function Check($what) { if ($LASTEXITCODE -ne 0) { Write-Host "FAILED: $what"; exit 1 } }

Remove-Item $out -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force "$out\classes", "$out\dex", "$out\jni" | Out-Null
Set-Location $root

if (-not $SkipRust) {
    Write-Host '== rust'
    # keep the build machine's folder names out of the binary (panic messages carry file paths)
    $ws = (Resolve-Path "$PSScriptRoot\..\..").Path
    $env:CARGO_ENCODED_RUSTFLAGS = "--remap-path-prefix=$ws=/heartnet" + [char]0x1f + "--remap-path-prefix=$env:USERPROFILE=/home/build"
    cargo ndk -t arm64-v8a --platform 30 -o "$out\jni" build --release --lib 2>&1 | ForEach-Object { "$_" } | Where-Object { $_ -match '^error|warning: unused|Finished' -or $_ -match '^\s+-->' }
    if (-not (Test-Path "$out\jni\arm64-v8a\libgallery.so")) { Write-Host 'FAILED: rust build'; exit 1 }
}

Write-Host '== java'
$sources = Get-ChildItem "$root\android\java" -Recurse -Filter *.java | ForEach-Object FullName
javac -source 8 -target 8 -bootclasspath $jar -Xlint:-options -d "$out\classes" $sources
Check 'javac'
$classes = Get-ChildItem "$out\classes" -Recurse -Filter *.class | ForEach-Object FullName
& "$bt\d8.bat" --lib $jar --min-api 30 --output "$out\dex" $classes
Check 'd8'

Write-Host '== manifest + package'
# The first cargo-apk build installed on the phone has versionCode 16777472, and Android refuses to
# go backwards, so numbering continues above it and goes up by one on every build.
$counterFile = "$root\version-code.txt"
$code = if (Test-Path $counterFile) { [int](Get-Content $counterFile) + 1 } else { 16777473 }
Set-Content $counterFile $code
& "$bt\aapt2.exe" compile --dir "$root\android\res" -o "$out\res.zip"
Check 'aapt2 compile'
& "$bt\aapt2.exe" link -o "$out\base.apk" -I $jar --manifest "$root\android\AndroidManifest.xml" `
    --min-sdk-version 30 --target-sdk-version 34 --version-code $code --version-name "1.1.$($code - 16777472)" "$out\res.zip"
Check 'aapt2'
Write-Host "versionCode $code"
Check 'aapt2'

foreach ($needed in "$out\dex\classes.dex", "$out\jni\arm64-v8a\libgallery.so", "$out\base.apk") {
    if (-not (Test-Path $needed)) { Write-Host "FAILED: missing $needed (do not use -SkipRust unless a build is already in place)"; exit 1 }
}
Add-Type -AssemblyName System.IO.Compression, System.IO.Compression.FileSystem
$zip = [System.IO.Compression.ZipFile]::Open("$out\base.apk", 'Update')
function AddFile($zip, $entry, $file) {
    $e = $zip.CreateEntry($entry, [System.IO.Compression.CompressionLevel]::Optimal)
    $s = $e.Open(); $b = [IO.File]::ReadAllBytes($file); $s.Write($b, 0, $b.Length); $s.Close()
}
AddFile $zip 'classes.dex' "$out\dex\classes.dex"
AddFile $zip 'lib/arm64-v8a/libgallery.so' "$out\jni\arm64-v8a\libgallery.so"
$zip.Dispose()

Write-Host '== align + sign'
& "$bt\zipalign.exe" -f -p 4 "$out\base.apk" "$out\aligned.apk"
Check 'zipalign'
if (-not $env:HEARTNET_KEYSTORE -or -not $env:HEARTNET_KEYSTORE_PASS) { Write-Host 'Set HEARTNET_KEYSTORE and HEARTNET_KEYSTORE_PASS first (see docs/SIGNING.md)'; exit 1 }
& "$bt\apksigner.bat" sign --ks $env:HEARTNET_KEYSTORE --ks-pass env:HEARTNET_KEYSTORE_PASS --key-pass env:HEARTNET_KEYSTORE_PASS --out $final "$out\aligned.apk"
Check 'apksigner'
& "$bt\apksigner.bat" verify $final
Check 'verify'

$f = Get-Item $final
Write-Host ("OK: {0} ({1:N1} MB)" -f $f.FullName, ($f.Length / 1MB))
