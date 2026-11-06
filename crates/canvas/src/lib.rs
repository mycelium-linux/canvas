//! Desktop Canvas runtime. Portable documents live in `canvas-workspace`.

pub mod backend;
pub mod background_picker;
#[cfg(feature = "lua-scenes")]
pub mod briefing;
pub mod bus;
pub mod chart;
pub mod compose;
pub mod control;
pub mod display;
#[cfg(unix)]
pub mod external_surface_binding;
pub mod hud;
pub mod interaction;
pub mod layout_tree;
pub mod loading_mark;
#[cfg(unix)]
pub mod loopback;
#[cfg(feature = "lua-scenes")]
pub mod lua_module;
#[cfg(feature = "lua-scenes")]
pub mod lua_scene;
pub mod map;
#[cfg(all(feature = "mcp", unix))]
pub mod mcp;
pub mod media;
#[cfg(feature = "lua-scenes")]
pub mod module_bus;
#[cfg(all(unix, feature = "lua-scenes"))]
pub mod native_switcher;
pub mod operational_views;
pub mod performance;
pub mod project_picker;
pub mod project_switcher;
pub mod reconcile;
pub mod runtime_config;
pub mod screensaver;
pub mod shell_dock;
pub mod shell_geometry;
pub mod shell_launcher;
pub mod spore_catalog;
#[cfg(feature = "lua-scenes")]
pub mod shell_presentation;
#[cfg(feature = "lua-scenes")]
pub mod stream_binding;
pub mod stream_contract;
pub mod stream_geometry;
pub mod theme_pack;
pub mod theme_picker;
pub mod view;
pub mod window_cycle;
pub mod workspace_mirror;
pub mod workspace_store;

/// Canvas's own shell row metrics.
///
/// `fpl_gfx::chrome` carries the metrics of a different shell -- a taller bar with
/// a 14 px label. Inheriting them made this shell inherit that shell's density,
/// and a bar is the one piece of chrome where four logical pixels decide whether
/// the desktop feels like a tool or a toolbar. Canvas owns its numbers; the
/// geometry it hands to Lua comes from here.
pub const DESKTOP_BAR_HEIGHT: u32 = 36;
pub const DESKTOP_BAR_TEXT_SIZE: u32 = 12;
pub const DESKTOP_BAR_PADDING: u32 = 10;
pub use canvas_workspace::*;
