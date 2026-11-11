#!/bin/sh
# Installs Janis on macOS without an Apple developer account behind it.
#
#   curl -fsSL https://raw.githubusercontent.com/obazin/janis/main/scripts/macos/install.sh | sh
#
# Downloads the DMG of the latest release (or of $JANIS_VERSION, e.g. 0.1.0)
# for this machine's architecture, copies Janis.app into /Applications (or
# ~/Applications when /Applications is not writable; override with
# $JANIS_INSTALL_DIRECTORY), verifies its signature, and clears the
# com.apple.quarantine flag. Janis is ad-hoc signed and not notarized, so a
# quarantined copy is refused by Gatekeeper on first launch; clearing the
# flag is exactly what "Open Anyway" in Privacy & Security does, done once
# here instead. Re-running the script upgrades an existing install in place.
set -eu

repository="obazin/janis"
application_name="Janis"

fail() {
    echo "error: $*" >&2
    exit 1
}

[ "$(uname -s)" = "Darwin" ] || fail "this installer is for macOS only"

case "$(uname -m)" in
    arm64) architecture="aarch64" ;;
    x86_64) architecture="x64" ;;
    *) fail "unsupported architecture: $(uname -m)" ;;
esac

# The version comes from the tag the "latest" release page redirects to,
# which needs no API token and is not rate limited like the REST API.
if [ -n "${JANIS_VERSION:-}" ]; then
    version="${JANIS_VERSION#v}"
else
    latest_url="$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/${repository}/releases/latest")" \
        || fail "could not reach github.com to find the latest release"
    case "$latest_url" in
        */releases/tag/v*) version="${latest_url##*/releases/tag/v}" ;;
        *) fail "no published release found at https://github.com/${repository}/releases" ;;
    esac
fi

if [ -n "${JANIS_INSTALL_DIRECTORY:-}" ]; then
    install_directory="$JANIS_INSTALL_DIRECTORY"
elif [ -w /Applications ]; then
    install_directory="/Applications"
else
    install_directory="$HOME/Applications"
fi
mkdir -p "$install_directory" || fail "cannot create $install_directory"
[ -w "$install_directory" ] || fail "$install_directory is not writable; set JANIS_INSTALL_DIRECTORY to a folder you own"

disk_image_name="${application_name}_${version}_${architecture}.dmg"
download_url="https://github.com/${repository}/releases/download/v${version}/${disk_image_name}"
application_path="${install_directory}/${application_name}.app"

work_directory="$(mktemp -d "${TMPDIR:-/tmp}/janis-install.XXXXXX")"
mount_point="${work_directory}/mount"
cleanup() {
    if [ -d "$mount_point" ]; then
        hdiutil detach "$mount_point" -quiet 2>/dev/null || hdiutil detach "$mount_point" -force -quiet 2>/dev/null || true
    fi
    rm -rf "$work_directory"
}
trap cleanup EXIT INT TERM

echo "Downloading ${application_name} ${version} (${architecture}) ..."
curl -fL --progress-bar -o "${work_directory}/${disk_image_name}" "$download_url" \
    || fail "download failed: $download_url"

echo "Opening the disk image ..."
mkdir -p "$mount_point"
# Images of earlier releases embed the license, so hdiutil asks for
# agreement before mounting them; the "Y" answers that prompt and is ignored
# otherwise (hdiutil cancels the prompt under -quiet, hence the redirect
# instead of the flag).
printf 'Y\n' | hdiutil attach -nobrowse -readonly -mountpoint "$mount_point" "${work_directory}/${disk_image_name}" >/dev/null \
    || fail "could not open the disk image"
[ -d "${mount_point}/${application_name}.app" ] || fail "the disk image does not contain ${application_name}.app"

# A running instance holds the binary open; ask it to quit first.
if pgrep -xq "$application_name"; then
    echo "Quitting the running ${application_name} ..."
    osascript -e "quit app \"${application_name}\"" >/dev/null 2>&1 || true
    attempts=0
    while pgrep -xq "$application_name" && [ "$attempts" -lt 20 ]; do
        sleep 0.5
        attempts=$((attempts + 1))
    done
    pgrep -xq "$application_name" && fail "${application_name} is still running; quit it and run the installer again"
fi

echo "Installing into ${install_directory} ..."
rm -rf "$application_path"
ditto "${mount_point}/${application_name}.app" "$application_path" || fail "could not copy ${application_name}.app into ${install_directory}"

codesign --verify --deep --strict "$application_path" 2>/dev/null \
    || fail "the installed copy fails signature verification; the download may be corrupt, try again"

xattr -dr com.apple.quarantine "$application_path" 2>/dev/null || true

echo "Installed ${application_name} ${version} at ${application_path}."
echo "Launch it from the Applications folder, Spotlight, or with: open -a ${application_name}"
