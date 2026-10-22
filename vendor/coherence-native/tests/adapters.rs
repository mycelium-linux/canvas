//! Contract regressions across native renderer boundaries.
use coherence_native::{Density, Mode, Tokens};

#[test]
fn invalid_host_metrics_are_rejected() {
    for value in [f32::NAN, f32::INFINITY, -1.0, 513.0] {
        let mut tokens = Tokens::new(Mode::Dark, Density::Compact);
        tokens.metrics.body = value;
        assert!(tokens.validate().is_err());
    }
    let mut tokens = Tokens::new(Mode::Light, Density::Comfortable);
    tokens.metrics.space[3] = 128.0;
    assert!(tokens.validate().is_err());
}

#[cfg(feature = "gfx")]
#[test]
fn invalid_gpu_override_retains_previous_theme() {
    let mut tokens = Tokens::new(Mode::Dark, Density::Compact);
    tokens.metrics.radius_small = f32::NAN;
    let mut theme = fpl_gfx::Theme::studio_dark();
    let before = theme.clone();
    assert!(coherence_native::gfx::apply(&tokens, &mut theme).is_err());
    assert_eq!(theme, before);
}

#[cfg(feature = "egui")]
#[test]
fn invalid_gui_override_retains_previous_style() {
    let mut tokens = Tokens::new(Mode::Dark, Density::Compact);
    tokens.metrics.body = f32::NAN;
    let mut style = egui::Style::default();
    let before = style.clone();
    assert!(coherence_native::egui::apply(&tokens, &mut style).is_err());
    assert_eq!(style, before);
}

#[cfg(all(feature = "gfx", feature = "egui"))]
#[test]
fn live_palette_overrides_reach_both_adapters_without_resetting_to_mode() {
    let mut tokens = Tokens::new(Mode::Dark, Density::Compact);
    tokens.colors.base.primary = [81, 140, 201];
    let mut gpu = fpl_gfx::Theme::studio_dark();
    coherence_native::gfx::apply(&tokens, &mut gpu).unwrap();
    let gui = coherence_native::egui::style(&tokens).unwrap();
    assert_eq!(gui.visuals.hyperlink_color.to_array(), [81, 140, 201, 255]);
    assert_eq!(
        gpu.color(fpl_gfx::ThemeRole::Primary),
        fpl_gfx::Color::from_srgb8(81, 140, 201, 255)
    );
    let mut scene = fpl_gfx::Scene::with_capacity(65);
    fpl_gfx::AnalogClock {
        hour: 10,
        minute: 9,
        second: 30.5,
    }
    .append(&mut scene, fpl_gfx::Rect::new(0.0, 0.0, 400.0, 300.0));
    assert_eq!(scene.len(), 65);
    assert!(scene.validate());
}

#[cfg(feature = "egui")]
#[test]
fn gui_behavior_flags_remain_owned_by_the_host() {
    let mut style = egui::Style::default();
    style.visuals.striped = true;
    style.visuals.slider_trailing_fill = true;
    coherence_native::egui::apply(&Tokens::new(Mode::Sepia, Density::Comfortable), &mut style)
        .unwrap();
    assert!(style.visuals.striped);
    assert!(style.visuals.slider_trailing_fill);
}
