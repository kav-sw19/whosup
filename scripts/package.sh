#!/bin/sh
# Build a universal macOS binary and package it for a release:
#   scripts/package.sh 0.2.0 [url-base]
# writes dist/whosup-<version>-macos-universal.tar.gz, its .sha256, and dist/whosup.rb
# (the Homebrew formula pointing at the release download, or at url-base if given).
set -eu
VERSION="${1:?usage: scripts/package.sh <version> [url-base]}"
URL_BASE="${2:-https://github.com/kav-sw19/whosup/releases/download/v$VERSION}"
NAME="whosup-$VERSION-macos-universal"

cargo build --release --locked --target aarch64-apple-darwin
cargo build --release --locked --target x86_64-apple-darwin

rm -rf dist && mkdir -p "dist/$NAME"
lipo -create -output "dist/$NAME/whosup" \
  target/aarch64-apple-darwin/release/whosup \
  target/x86_64-apple-darwin/release/whosup
cp README.md LICENSE "dist/$NAME/"
tar -C dist -czf "dist/$NAME.tar.gz" "$NAME"
SHA=$(shasum -a 256 "dist/$NAME.tar.gz" | cut -d' ' -f1)
echo "$SHA  $NAME.tar.gz" > "dist/$NAME.tar.gz.sha256"

sed -e "s|@VERSION@|$VERSION|g" -e "s|@URL@|$URL_BASE/$NAME.tar.gz|g" -e "s|@SHA256@|$SHA|g" \
  scripts/whosup.rb.in > dist/whosup.rb

lipo -info "dist/$NAME/whosup"
echo "dist/$NAME.tar.gz  sha256 $SHA"
