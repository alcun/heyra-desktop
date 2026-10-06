#!/bin/zsh
# Build, sign with Developer ID, notarize, staple, publish to GitHub and bump the
# Homebrew cask. Needs: a Developer ID Application certificate, notarytool
# credentials saved as "heyra" (xcrun notarytool store-credentials heyra ...),
# gh logged in, and the tap checked out at ../homebrew-tap.
set -e
cd "$(dirname "$0")"
VERSION=$(awk -F'"' '/^version/ {print $2; exit}' Cargo.toml)
TAP=../homebrew-tap/Casks/heyra.rb
[[ -z $(git status --porcelain) ]] || { echo "commit first"; exit 1; }

HEYRA_SIGN="${HEYRA_SIGN:-Developer ID Application}" ./bundle.sh
OUT=$(mktemp -d)
ditto -c -k --keepParent target/Heyra.app "$OUT/Heyra.zip"
xcrun notarytool submit "$OUT/Heyra.zip" --keychain-profile heyra --wait
xcrun stapler staple target/Heyra.app
rm "$OUT/Heyra.zip"
ditto -c -k --keepParent target/Heyra.app "$OUT/Heyra.zip"
spctl --assess --type execute -v target/Heyra.app

git tag "v$VERSION"
git push origin main "v$VERSION"
gh release create "v$VERSION" "$OUT/Heyra.zip" --title "Heyra $VERSION" --generate-notes

SHA=$(shasum -a 256 "$OUT/Heyra.zip" | cut -d' ' -f1)
sed -i '' -e "s/^  version \".*\"/  version \"$VERSION\"/" -e "s/^  sha256 \".*\"/  sha256 \"$SHA\"/" "$TAP"
git -C "$(dirname "$TAP")" commit -qam "heyra $VERSION"
git -C "$(dirname "$TAP")" push -q
echo "Released $VERSION: brew upgrade --cask heyra"
