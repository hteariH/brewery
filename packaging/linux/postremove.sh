#!/bin/sh
# deb postrm / rpm %postun: refresh the menu and icon caches after the files are gone.
# Also runs on upgrades and (deb) purge; refreshing again is harmless.
set -e

if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database -q /usr/share/applications || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -q -t -f /usr/share/icons/hicolor || true
fi
exit 0
