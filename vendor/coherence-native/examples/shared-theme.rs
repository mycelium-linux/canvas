//! One appearance snapshot driving both independent renderer adapters.
use coherence_native::{Density, Mode, Tokens};

fn main() {
    let tokens = Tokens::new(Mode::Dark, Density::Compact);
    let mut gpu = fpl_gfx::Theme::studio_dark();
    coherence_native::gfx::apply(&tokens, &mut gpu).expect("valid Coherence tokens");
    gpu.validate().expect("generated Coherence GPU theme");
    let gui = coherence_native::egui::style(&tokens).expect("valid Coherence tokens");
    println!(
        "Coherence {:?}/{:?}: GPU primary {:?}; GUI panel {:?}; control height {}",
        tokens.mode,
        tokens.density,
        gpu.color(fpl_gfx::ThemeRole::Primary),
        gui.visuals.panel_fill,
        gui.spacing.interact_size.y
    );
}
