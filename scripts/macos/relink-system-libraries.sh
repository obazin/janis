#!/bin/sh
# Points a binary built in the Nix dev shell at the system's own libraries.
#
# The dev shell links a few libraries from the Nix store. A signed application
# bundle refuses to load them — their signature is not the bundle's — and they
# exist on no other machine anyway. Each has an equivalent every macOS ships,
# at the same name under /usr/lib; this rewrites the references to it.
#
# Usage: relink-system-libraries.sh <binary>
set -eu

binary="$1"

otool -L "$binary" | awk '/^\t\/nix\/store\// { print $1 }' | while read -r library; do
    system_library="/usr/lib/$(basename "$library")"
    echo "relink $(basename "$library") -> $system_library"
    install_name_tool -change "$library" "$system_library" "$binary"
done
