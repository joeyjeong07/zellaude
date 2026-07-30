use zellij_tile::prelude::{PaletteColor, StyleDeclaration, Styling};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SegmentStyle {
    pub base: PaletteColor,
    pub background: PaletteColor,
    pub emphasis: [PaletteColor; 4],
}

impl SegmentStyle {
    pub fn emphasis(self, index: usize) -> PaletteColor {
        self.emphasis.get(index).copied().unwrap_or(self.base)
    }
}

impl From<StyleDeclaration> for SegmentStyle {
    fn from(style: StyleDeclaration) -> Self {
        Self {
            base: style.base,
            background: style.background,
            emphasis: [
                style.emphasis_0,
                style.emphasis_1,
                style.emphasis_2,
                style.emphasis_3,
            ],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BarTheme {
    pub surface: SegmentStyle,
    pub prefix: SegmentStyle,
    pub settings_prefix: SegmentStyle,
    pub active_tab: SegmentStyle,
    pub inactive_tab: SegmentStyle,
    pub frame_selected: SegmentStyle,
    pub highlight: SegmentStyle,
    pub success: SegmentStyle,
    pub error: SegmentStyle,
    pub flash: SegmentStyle,
}

impl BarTheme {
    pub fn from_styling(styling: &Styling) -> Self {
        let surface = SegmentStyle::from(styling.text_unselected);
        let error = SegmentStyle::from(styling.exit_code_error);

        Self {
            surface,
            prefix: SegmentStyle::from(styling.ribbon_unselected),
            settings_prefix: SegmentStyle::from(styling.ribbon_selected),
            active_tab: SegmentStyle::from(styling.text_selected),
            inactive_tab: surface,
            frame_selected: SegmentStyle::from(styling.frame_selected),
            highlight: SegmentStyle::from(styling.frame_highlight),
            success: SegmentStyle::from(styling.exit_code_success),
            error,
            // A permission request needs a visible background pulse. Zellij's
            // error declaration stores the semantic error color in `base`, so
            // invert it into the background while retaining theme-derived text.
            flash: SegmentStyle {
                base: surface.base,
                background: error.base,
                emphasis: error.emphasis,
            },
        }
    }
}

impl Default for BarTheme {
    fn default() -> Self {
        Self::from_styling(&Styling::default())
    }
}

pub fn foreground(color: PaletteColor) -> String {
    match color {
        PaletteColor::Rgb((r, g, b)) => format!("\x1b[38;2;{r};{g};{b}m"),
        PaletteColor::EightBit(index) => format!("\x1b[38;5;{index}m"),
    }
}

pub fn background(color: PaletteColor) -> String {
    match color {
        PaletteColor::Rgb((r, g, b)) => format!("\x1b[48;2;{r};{g};{b}m"),
        PaletteColor::EightBit(index) => format!("\x1b[48;5;{index}m"),
    }
}

pub fn exact_rgb(color: PaletteColor) -> Option<(u8, u8, u8)> {
    match color {
        PaletteColor::Rgb(rgb) => Some(rgb),
        PaletteColor::EightBit(_) => None,
    }
}
