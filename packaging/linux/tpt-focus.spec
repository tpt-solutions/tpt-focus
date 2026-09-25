# tpt-focus spec for RPM-based distributions.
# Build: scripts/linux/package-rpm.sh (expects scripts/linux/stage.sh output).

Name:           tpt-focus
Version:        %{_version}
Release:        1%{?dist}
Summary:        Unified notification & focus center

License:        MIT OR Apache-2.0
URL:            https://github.com/tpt-solutions/tpt-focus
Source0:        https://github.com/tpt-solutions/tpt-focus/archive/v%{version}.tar.gz

BuildArch:      x86_64
Recommends:     (gnome-shell-extension-appindicator if gnome-shell)

%description
Rule-based notification manager: allows, mutes or batches every
notification, keeps a searchable local history, and acts as the
org.freedesktop.Notifications daemon on Linux.

%prep
# The staged install tree is produced by scripts/linux/stage.sh; the spec
# packages it as-is.

%install
rm -rf %{buildroot}
cp -a %{_builddir}/../stage/usr %{buildroot}/usr 2>/dev/null || {
    mkdir -p %{buildroot}/usr
    cp -a %{_sourcedir}/../build/linux/stage/usr %{buildroot}/
}

%files
%{_bindir}/tpt-focus
%{_bindir}/tpt-focus-tray
%{_userunitdir}/tpt-focus.service
%{_datadir}/applications/tpt-focus.desktop
%{_datadir}/metainfo/tpt-focus.appdata.xml
%doc README.md

%post
systemctl --user daemon-reload 2>/dev/null || true

%postun
systemctl --user daemon-reload 2>/dev/null || true
