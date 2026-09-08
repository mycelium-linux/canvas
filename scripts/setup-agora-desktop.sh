#!/bin/sh
# Run on the Linux desktop host from the Dock repository root.
set -eu
test "$(uname -s)" = Linux || { echo 'Run this script on the Linux host.' >&2; exit 1; }
test -f config/linux/sway/themes/dock.conf
test -f config/linux/sway/themes/studio.conf
sudo -n env DEBIAN_FRONTEND=noninteractive apt-get install -y sway swaybg wayvnc foot grim wtype cargo openssl curl xz-utils

install -d -m 700 "$HOME/.config/dock/sway/themes" "$HOME/.config/dock/hypr" "$HOME/.config/dock/wayvnc" "$HOME/.local/bin"
install -m 644 config/linux/sway/config "$HOME/.config/dock/sway/config"
for project in dock studio; do
    install -m 644 "config/linux/sway/themes/$project.conf" "$HOME/.config/dock/sway/themes/$project.conf"
    install -m 644 "config/linux/themes/foot-$project.ini" "$HOME/.config/dock/hypr/foot-$project.ini"
done

# Use native Firefox with private project profiles: the distribution's Snap
# cannot be assumed to allow our separate Wayland runtime directory.
if ! test -x "$HOME/.local/lib/dock-browser/firefox/firefox"; then
    dock_download=$(mktemp -d)
    trap 'rm -rf "$dock_download"' EXIT HUP INT TERM
    curl -fL 'https://download.mozilla.org/?product=firefox-latest-ssl&os=linux64&lang=en-US' -o "$dock_download/firefox.tar.xz"
    install -d "$HOME/.local/lib/dock-browser"
    tar -xJf "$dock_download/firefox.tar.xz" -C "$HOME/.local/lib/dock-browser"
fi

# Re-running preserves the existing authentication identity and credential.
umask 077
dock_vnc_config="$HOME/.config/dock/wayvnc"
if ! test -f "$dock_vnc_config/password"; then
    openssl rand -base64 24 > "$dock_vnc_config/password"
fi
if ! test -f "$dock_vnc_config/tls-key.pem"; then
    openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:secp384r1 -sha384 -days 365 -nodes \
        -keyout "$dock_vnc_config/tls-key.pem" -out "$dock_vnc_config/tls-cert.pem" \
        -subj /CN=localhost -addext subjectAltName=DNS:localhost,IP:127.0.0.1
fi
if ! test -f "$dock_vnc_config/config"; then
    {
        printf 'address=127.0.0.1\nport=5901\nenable_auth=true\nrelax_encryption=true\nusername=dock\n'
        printf 'password=%s\n' "$(cat "$dock_vnc_config/password")"
        printf 'private_key_file=%s/tls-key.pem\ncertificate_file=%s/tls-cert.pem\n' "$dock_vnc_config" "$dock_vnc_config"
    } > "$dock_vnc_config/config"
fi
chmod 600 "$dock_vnc_config/password" "$dock_vnc_config/config" "$dock_vnc_config/tls-key.pem"
cargo build --release -p dock-linux
# Cargo owns where artifacts land: CARGO_TARGET_DIR, or build.target-dir in a cargo
# config, can point it away from ./target. Ask cargo rather than assume.
target=$(cargo metadata --format-version 1 --no-deps | jq -r .target_directory)
install -m 755 "$target/release/dock-linux" "$HOME/.local/bin/dock-linux.next"
mv -f "$HOME/.local/bin/dock-linux.next" "$HOME/.local/bin/dock-linux"
"$HOME/.local/bin/dock-linux" desktop-start
printf 'Desktop ready. Forward local 15901 to Agora localhost:5901 over SSH. VNC account dock; credential stays in ~/.config/dock/wayvnc/password.\n'
