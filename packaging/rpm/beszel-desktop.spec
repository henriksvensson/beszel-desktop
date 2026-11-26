Name:           beszel-desktop
Version:        0.4.0
Release:        1%{?dist}
Summary:        Live Beszel metrics daemon, CLI, and Wayland desktop shell widgets

License:        MIT
URL:            https://github.com/henriksvensson/beszel-desktop
Source0:        %{url}/archive/refs/tags/v%{version}/%{name}-%{version}.tar.gz

BuildRequires:  cargo
BuildRequires:  rustc >= 1.85
BuildRequires:  systemd-rpm-macros

%description
Beszel Desktop runs a lightweight per-user Rust daemon (beszel-relay) that
streams live (~1 Hz) CPU, memory, and per-core metrics from a Beszel hub over a
user-only Unix socket, paired with the beszel-ctl CLI and plugins for
DankMaterialShell, Noctalia Shell v5, and Waybar.

%prep
%autosetup -n %{name}-%{version}

%build
cargo build --release --locked --workspace

%check
cargo test --locked --workspace

%install
install -Dm755 target/release/beszel-relay %{buildroot}%{_bindir}/beszel-relay
install -Dm755 target/release/beszel-ctl %{buildroot}%{_bindir}/beszel-ctl

install -dm755 %{buildroot}%{_userunitdir}
sed 's|%h/.local/bin/beszel-relay|%{_bindir}/beszel-relay|g' \
    packaging/systemd/beszel-relay.service \
    > %{buildroot}%{_userunitdir}/beszel-relay.service

install -Dm644 packaging/config.example.toml %{buildroot}%{_docdir}/%{name}/config.example.toml
install -dm755 %{buildroot}%{_datadir}/%{name}/shells
cp -r shells/dms shells/noctalia shells/waybar %{buildroot}%{_datadir}/%{name}/shells/

%files
%license LICENSE
%doc README.md packaging/config.example.toml
%{_bindir}/beszel-relay
%{_bindir}/beszel-ctl
%{_userunitdir}/beszel-relay.service
%{_datadir}/%{name}/shells/

%changelog
* Thu Nov 26 2026 Henrik Svensson <1612710+henriksvensson@users.noreply.github.com> - 0.4.0-1
- Expanded live metrics (GPU, network RX/TX, root & extra disks, storage pools, containers, failed systemd services, pending updates, battery state, OS/CPU details) and configurable 2-column Noctalia panel & bar settings

* Thu Nov 26 2026 Henrik Svensson <1612710+henriksvensson@users.noreply.github.com> - 0.3.0-1
- Add `beszel-ctl update`, Noctalia Pin-to-bar, scroll-to-switch, and monochrome bar icons

* Thu Nov 26 2026 Henrik Svensson <1612710+henriksvensson@users.noreply.github.com> - 0.2.0-1
- Add `beszel-ctl init` interactive setup wizard, checksum-verified installer script, and Noctalia onboarding card

* Thu Nov 26 2026 Henrik Svensson <1612710+henriksvensson@users.noreply.github.com> - 0.1.0-1
- Initial release
