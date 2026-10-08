# Native Canvas Debian payload

`tools/canvas-deb` packages an owner-built `canvas-linux` ELF. Debian's
`dpkg-shlibdeps` derives library dependencies; architecture comes from ELF and
version comes from this workspace. It refuses cross-host dependency resolution
without a target sysroot. Native amd64 is the first qualification target.

```sh
cargo check --manifest-path tools/canvas-deb/Cargo.toml
cargo test --manifest-path tools/canvas-deb/Cargo.toml
cargo build --locked -p canvas-linux --features compositor
cargo run --manifest-path tools/canvas-deb/Cargo.toml -- \
  target/debug/canvas-linux /new/package/output
```

Requires Linux, `dpkg-dev`, and the matching compositor runtime/development
libraries. The cursor theme is an explicit runtime dependency (`adwaita-icon-theme`),
because library scanning cannot infer asset requirements. Inspect with
`dpkg-deb --info` and `--contents`. The payload installs
only `/usr/bin/canvas-linux`; there are no maintainer scripts, conffiles, service
units, start/enable actions or credentials. Image assembly owns the explicit
native/headless compositor unit, session/DRM permissions and local health binding.
Build provenance remains the build owner's responsibility. A binary copied from
an unrelated cache is not a qualified release.

The fungOS publisher accepts this archive alongside Unibus's existing
`linux_core_deb` output. Mycelium coordinates exact-version updates and recovery;
APT/dpkg owns package files. Do not simultaneously activate the same executable
through Mycelium's native binary updater. Packaging code is MIT OR Apache-2.0;
the Canvas payload retains the application license declared by its owner.
