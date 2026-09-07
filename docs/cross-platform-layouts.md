# Cross-platform project layouts

Dock owns one semantic layout policy in `dock-core`. A project surface has a
stable identity and a role such as browser or tool. The default plan gives the
first browser a large primary region and assigns every other surface to one
secondary stack region. With no browser, the first attached surface is primary.

The plan is independent of window-system identifiers. On macOS, `dock-mac`
resolves explicit project bindings, reads the selected display's visible frame,
and asks the separately installed Dock Control helper to apply each frame via
Accessibility. Secondary windows overlap in the stack region and remain
selectable through Dock recall. Windows outside the project binding are never
moved.

Sway compiles the same primary/stack regions into compositor containers, where
the secondary region can be genuinely tabbed. Alfalfa remains the source of
project and resource identity; it does not own window geometry.

Dock intentionally does not use private macOS Spaces APIs. Project recall and
tiling operate on visible desktop windows through public AppKit and
Accessibility APIs, preserving the independently installed helper's TCC
identity across normal Dock GUI rebuilds.
