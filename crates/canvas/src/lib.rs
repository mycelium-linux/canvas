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

pub const DESKTOP_BAR_HEIGHT: u32 = fpl_gfx::chrome::BAR_HEIGHT;
pub use canvas_workspace::*;
