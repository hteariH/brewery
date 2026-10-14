#!/bin/sh
# deb postinst / rpm %post: let menus pick up the desktop file and the icons.
# Both tools are optional (minimal containers lack them), so failures don't fail the install.
set -e

if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database -q /usr/share/applications || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -q -t -f /usr/share/icons/hicolor || true
fi
exit 0
