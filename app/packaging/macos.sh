#!/usr/bin/env bash
# The macOS disk image from a release build: Anvil.app beside a link to
# Applications, to drag across.
#
#   packaging/macos.sh <label> <Anvil.app> <out dir>
set -euo pipefail
umask 022

label=$1 app=$2 out=$3
mkdir -p "$out"
stage=$(mktemp -d)/Anvil
mkdir -p "$stage"
cp -R "$app" "$stage/"
ln -s /Applications "$stage/Applications"
hdiutil create -volname "Anvil" -srcfolder "$stage" -ov -format UDZO \
  "$out/Anvil-${label}-macos.dmg"
ls -l "$out"
