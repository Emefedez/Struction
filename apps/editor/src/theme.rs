//! The Struction editor look: graphite surfaces, one amber accent, Inter and JetBrains Mono.
//! Every egui default that makes an app read as "an egui demo" is replaced here, so panels and
//! tools only choose layout and never colors or fonts.

use std::sync::Arc;

use bevy_egui::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, RichText,
    Shadow, Stroke, TextStyle, Vec2, style::WidgetVisuals,
};

pub const BASE: Color32 = Color32::from_rgb(0x11, 0x13, 0x17);
pub const PANEL: Color32 = Color32::from_rgb(0x17, 0x1a, 0x1f);
const SURFACE: Color32 = Color32::from_rgb(0x20, 0x24, 0x2b);
const HOVER: Color32 = Color32::from_rgb(0x2a, 0x2f, 0x38);
const PRESSED: Color32 = Color32::from_rgb(0x34, 0x3a, 0x45);
const BORDER: Color32 = Color32::from_rgb(0x26, 0x2a, 0x31);
const TEXT: Color32 = Color32::from_rgb(0xd7, 0xdb, 0xe2);
pub const MUTED: Color32 = Color32::from_rgb(0x8a, 0x91, 0x9e);
/// Semantic roles stay consistent across the hierarchy, inspectors and legends.
pub const ACTOR: Color32 = Color32::from_rgb(0x78, 0xc8, 0xf0);
pub const MASTER: Color32 = Color32::from_rgb(0xf2, 0xc1, 0x6a);
pub const WARD: Color32 = Color32::from_rgb(0x76, 0xd4, 0xb2);
pub const DEFINITION: Color32 = Color32::from_rgb(0xbd, 0xa1, 0xf0);
pub const ASSET: Color32 = Color32::from_rgb(0xe8, 0xad, 0x8b);
pub const ACCENT: Color32 = Color32::from_rgb(0xf2, 0xa9, 0x3b);
/// Axis colors shared by gizmos and vector fields.
pub const AXES: [Color32; 3] = [
    Color32::from_rgb(0xe8, 0x5d, 0x5d),
    Color32::from_rgb(0x7c, 0xc6, 0x5a),
    Color32::from_rgb(0x5a, 0x9b, 0xe8),
];

const SEMIBOLD: &str = "semibold";

pub fn apply(ctx: &egui::Context) {
    ctx.set_fonts(fonts());
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Small, FontId::proportional(11.5)),
            (TextStyle::Body, FontId::proportional(13.5)),
            (TextStyle::Button, FontId::proportional(13.5)),
            (TextStyle::Heading, FontId::new(16.0, semibold())),
            (TextStyle::Monospace, FontId::monospace(12.5)),
        ]
        .into();

        let spacing = &mut style.spacing;
        spacing.item_spacing = Vec2::new(8.0, 6.0);
        spacing.button_padding = Vec2::new(10.0, 4.0);
        spacing.interact_size = Vec2::new(40.0, 26.0);
        spacing.indent = 14.0;
        spacing.window_margin = Margin::same(12);
        spacing.menu_margin = Margin::same(6);

        let visuals = &mut style.visuals;
        *visuals = egui::Visuals::dark();
        visuals.panel_fill = PANEL;
        visuals.window_fill = PANEL;
        visuals.extreme_bg_color = BASE;
        visuals.faint_bg_color = SURFACE;
        visuals.code_bg_color = BASE;
        visuals.window_stroke = Stroke::new(1.0, BORDER);
        visuals.window_corner_radius = CornerRadius::same(10);
        visuals.menu_corner_radius = CornerRadius::same(8);
        let shadow = Shadow {
            offset: [0, 8],
            blur: 24,
            spread: 0,
            color: Color32::from_black_alpha(110),
        };
        visuals.window_shadow = shadow;
        visuals.popup_shadow = shadow;
        visuals.selection.bg_fill = ACCENT.linear_multiply(0.28);
        visuals.selection.stroke = Stroke::new(1.0, ACCENT);
        visuals.hyperlink_color = ACCENT;
        visuals.warn_fg_color = Color32::from_rgb(0xe8, 0xc1, 0x5a);
        visuals.error_fg_color = Color32::from_rgb(0xf0, 0x65, 0x5a);
        visuals.indent_has_left_vline = true;
        visuals.collapsing_header_frame = false;
        visuals.slider_trailing_fill = true;

        // Idle widgets are flat fills; borders appear only for focus and interaction.
        let widget = |fill: Color32, text: Color32, stroke: Stroke| WidgetVisuals {
            bg_fill: fill,
            weak_bg_fill: fill,
            bg_stroke: stroke,
            corner_radius: CornerRadius::same(5),
            fg_stroke: Stroke::new(1.0, text),
            expansion: 0.0,
        };
        let widgets = &mut visuals.widgets;
        widgets.noninteractive = widget(PANEL, TEXT, Stroke::new(1.0, BORDER));
        widgets.inactive = widget(SURFACE, TEXT, Stroke::NONE);
        widgets.hovered = widget(HOVER, Color32::WHITE, Stroke::NONE);
        widgets.active = widget(PRESSED, Color32::WHITE, Stroke::new(1.0, ACCENT));
        widgets.open = widget(HOVER, Color32::WHITE, Stroke::NONE);
    });
}

fn semibold() -> FontFamily {
    FontFamily::Name(SEMIBOLD.into())
}

fn fonts() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    for (name, bytes) in [
        (
            "inter",
            include_bytes!("../assets/fonts/Inter-Medium.ttf").as_slice(),
        ),
        (
            SEMIBOLD,
            include_bytes!("../assets/fonts/Inter-SemiBold.ttf").as_slice(),
        ),
        (
            "jetbrains-mono",
            include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf").as_slice(),
        ),
    ] {
        fonts
            .font_data
            .insert(name.into(), Arc::new(FontData::from_static(bytes)));
    }
    // egui's bundled fonts stay as fallbacks for symbols and emoji.
    let fallbacks = fonts.families[&FontFamily::Proportional].clone();
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "inter".into());
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .insert(0, "jetbrains-mono".into());
    fonts.families.insert(
        semibold(),
        std::iter::once(SEMIBOLD.to_owned())
            .chain(fallbacks)
            .collect(),
    );
    fonts
}

/// Small, letter-spaced caps that title panels and sections.
pub fn section(text: &str) -> RichText {
    RichText::new(text.to_uppercase())
        .font(FontId::new(10.5, semibold()))
        .extra_letter_spacing(1.2)
        .color(MUTED)
}
