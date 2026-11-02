#!/bin/bash
# Writes the winget manifests for a release of amaye15/dtk into packaging/winget/.
# Needs the gh CLI. Public release data only.
#
#   scripts/gen-winget.sh v0.4.0
#
# The files follow the layout of microsoft/winget-pkgs: manifests/a/amaye15/dtk/<version>/.
# Copy that folder into a fork of winget-pkgs and open a pull request to publish the package.
set -euo pipefail
repo="amaye15/dtk"
tag="${1:-$(gh release view -R "$repo" --json tagName -q .tagName)}"
ver="${tag#v}"
target="x86_64-pc-windows-msvc"
asset="dtk-$tag-$target.zip"
sum="$(gh release download "$tag" -R "$repo" -p "$asset.sha256" -O - | awk '{print toupper($1)}')"
date="$(gh release view "$tag" -R "$repo" --json publishedAt -q '.publishedAt[:10]')"
out="$(dirname "$0")/../packaging/winget/manifests/a/amaye15/dtk/$ver"
mkdir -p "$out"
schema="https://aka.ms/winget-manifest"

cat > "$out/amaye15.dtk.yaml" <<YAML
# yaml-language-server: \$schema=$schema.version.1.6.0.schema.json
PackageIdentifier: amaye15.dtk
PackageVersion: $ver
DefaultLocale: en-US
ManifestType: version
ManifestVersion: 1.6.0
YAML

cat > "$out/amaye15.dtk.installer.yaml" <<YAML
# yaml-language-server: \$schema=$schema.installer.1.6.0.schema.json
PackageIdentifier: amaye15.dtk
PackageVersion: $ver
InstallerType: zip
NestedInstallerType: portable
NestedInstallerFiles:
  - RelativeFilePath: dtk.exe
    PortableCommandAlias: dtk
Commands:
  - dtk
ReleaseDate: $date
Installers:
  - Architecture: x64
    InstallerUrl: https://github.com/$repo/releases/download/$tag/$asset
    InstallerSha256: $sum
ManifestType: installer
ManifestVersion: 1.6.0
YAML

cat > "$out/amaye15.dtk.locale.en-US.yaml" <<YAML
# yaml-language-server: \$schema=$schema.defaultLocale.1.6.0.schema.json
PackageIdentifier: amaye15.dtk
PackageVersion: $ver
PackageLocale: en-US
Publisher: amaye15
PublisherUrl: https://github.com/amaye15
PublisherSupportUrl: https://github.com/$repo/issues
PackageName: dtk
PackageUrl: https://github.com/$repo
License: MIT
LicenseUrl: https://github.com/$repo/blob/main/LICENSE-MIT
ShortDescription: Data token killer. A Claude Code hook that turns big tables, CSV, JSON and logs into lean summaries.
Description: dtk is a Claude Code hook. It finds big tables in command output (CSV, JSON, JSON lines, logfmt, access logs, polars and psql tables) and replaces them with a short summary that covers every row. It works with rtk.
Moniker: dtk
Tags:
  - claude-code
  - cli
  - hook
  - llm
  - rust
  - tokens
ReleaseNotesUrl: https://github.com/$repo/releases/tag/$tag
ManifestType: defaultLocale
ManifestVersion: 1.6.0
YAML
echo "wrote $out"
