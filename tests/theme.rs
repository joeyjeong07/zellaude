#![allow(dead_code)]

extern crate self as zellij_tile;

pub mod prelude {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum PaletteColor {
        Rgb((u8, u8, u8)),
        EightBit(u8),
    }

    impl Default for PaletteColor {
        fn default() -> Self {
            Self::EightBit(0)
        }
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct StyleDeclaration {
        pub base: PaletteColor,
        pub background: PaletteColor,
        pub emphasis_0: PaletteColor,
        pub emphasis_1: PaletteColor,
        pub emphasis_2: PaletteColor,
        pub emphasis_3: PaletteColor,
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct Styling {
        pub text_unselected: StyleDeclaration,
        pub text_selected: StyleDeclaration,
        pub ribbon_unselected: StyleDeclaration,
        pub ribbon_selected: StyleDeclaration,
        pub frame_selected: StyleDeclaration,
        pub frame_highlight: StyleDeclaration,
        pub exit_code_success: StyleDeclaration,
        pub exit_code_error: StyleDeclaration,
    }
}

#[path = "../src/theme.rs"]
mod theme;

use prelude::{PaletteColor, StyleDeclaration, Styling};
use theme::{BarTheme, SegmentStyle};

fn rgb(r: u8, g: u8, b: u8) -> PaletteColor {
    PaletteColor::Rgb((r, g, b))
}

fn declaration(colors: [PaletteColor; 6]) -> StyleDeclaration {
    StyleDeclaration {
        base: colors[0],
        background: colors[1],
        emphasis_0: colors[2],
        emphasis_1: colors[3],
        emphasis_2: colors[4],
        emphasis_3: colors[5],
    }
}

fn gruvbox_dark() -> Styling {
    let text_unselected = declaration([
        rgb(251, 241, 199),
        rgb(60, 56, 54),
        rgb(214, 93, 14),
        rgb(104, 157, 106),
        rgb(152, 151, 26),
        rgb(177, 98, 134),
    ]);
    let text_selected = declaration([
        rgb(251, 241, 199),
        rgb(80, 73, 69),
        rgb(214, 93, 14),
        rgb(104, 157, 106),
        rgb(152, 151, 26),
        rgb(177, 98, 134),
    ]);
    let ribbon_selected = declaration([
        rgb(60, 56, 54),
        rgb(152, 151, 26),
        rgb(204, 36, 29),
        rgb(214, 93, 14),
        rgb(177, 98, 134),
        rgb(69, 133, 136),
    ]);
    let ribbon_unselected = declaration([
        rgb(60, 56, 54),
        rgb(235, 219, 178),
        rgb(204, 36, 29),
        rgb(251, 241, 199),
        rgb(69, 133, 136),
        rgb(177, 98, 134),
    ]);
    let frame_selected = declaration([
        rgb(152, 151, 26),
        PaletteColor::EightBit(0),
        rgb(214, 93, 14),
        rgb(104, 157, 106),
        rgb(177, 98, 134),
        PaletteColor::EightBit(0),
    ]);
    let frame_highlight = declaration([
        rgb(214, 93, 14),
        PaletteColor::EightBit(0),
        rgb(177, 98, 134),
        rgb(214, 93, 14),
        rgb(214, 93, 14),
        rgb(214, 93, 14),
    ]);
    let exit_code_success = declaration([
        rgb(152, 151, 26),
        PaletteColor::EightBit(0),
        rgb(104, 157, 106),
        rgb(60, 56, 54),
        rgb(177, 98, 134),
        rgb(69, 133, 136),
    ]);
    let exit_code_error = declaration([
        rgb(204, 36, 29),
        PaletteColor::EightBit(0),
        rgb(215, 153, 33),
        PaletteColor::EightBit(0),
        PaletteColor::EightBit(0),
        PaletteColor::EightBit(0),
    ]);

    Styling {
        text_unselected,
        text_selected,
        ribbon_unselected,
        ribbon_selected,
        frame_selected,
        frame_highlight,
        exit_code_success,
        exit_code_error,
    }
}

#[test]
fn ansi_output_preserves_rgb_and_terminal_index_colors() {
    assert_eq!(
        theme::foreground(rgb(251, 241, 199)),
        "\x1b[38;2;251;241;199m"
    );
    assert_eq!(theme::background(rgb(60, 56, 54)), "\x1b[48;2;60;56;54m");
    assert_eq!(
        theme::foreground(PaletteColor::EightBit(15)),
        "\x1b[38;5;15m"
    );
    assert_eq!(theme::background(PaletteColor::EightBit(0)), "\x1b[48;5;0m");
    assert_eq!(
        theme::foreground(PaletteColor::EightBit(255)),
        "\x1b[38;5;255m"
    );
    assert_eq!(theme::exact_rgb(rgb(251, 241, 199)), Some((251, 241, 199)));
    assert_eq!(theme::exact_rgb(PaletteColor::EightBit(15)), None);
}

#[test]
fn gruvbox_dark_maps_its_resolved_zellij_declarations_to_bar_roles() {
    let styling = gruvbox_dark();
    let bar = BarTheme::from_styling(&styling);

    assert_eq!(bar.surface, SegmentStyle::from(styling.text_unselected));
    assert_eq!(bar.prefix, SegmentStyle::from(styling.ribbon_unselected));
    assert_eq!(
        bar.settings_prefix,
        SegmentStyle::from(styling.ribbon_selected)
    );
    assert_eq!(bar.active_tab, SegmentStyle::from(styling.text_selected));
    assert_eq!(
        bar.inactive_tab,
        SegmentStyle::from(styling.text_unselected)
    );
    assert_eq!(
        bar.frame_selected,
        SegmentStyle::from(styling.frame_selected)
    );
    assert_eq!(bar.highlight, SegmentStyle::from(styling.frame_highlight));

    assert_eq!(bar.surface.base, rgb(251, 241, 199));
    assert_eq!(bar.surface.background, rgb(60, 56, 54));
    assert_eq!(bar.prefix.base, rgb(60, 56, 54));
    assert_eq!(bar.prefix.background, rgb(235, 219, 178));
    assert_eq!(bar.settings_prefix.background, rgb(152, 151, 26));
    assert_eq!(bar.active_tab.background, rgb(80, 73, 69));
    assert_eq!(bar.inactive_tab.background, rgb(60, 56, 54));
}

#[test]
fn gruvbox_permission_flash_uses_theme_error_and_text_colors() {
    let styling = gruvbox_dark();
    let bar = BarTheme::from_styling(&styling);

    assert_eq!(bar.error.background, PaletteColor::EightBit(0));
    assert_eq!(bar.flash.base, styling.text_unselected.base);
    assert_eq!(bar.flash.background, styling.exit_code_error.base);
    assert_eq!(bar.flash.background, rgb(204, 36, 29));
}

#[test]
fn ansi_themes_remain_indexed_end_to_end() {
    let indexed = declaration([
        PaletteColor::EightBit(15),
        PaletteColor::EightBit(0),
        PaletteColor::EightBit(1),
        PaletteColor::EightBit(2),
        PaletteColor::EightBit(3),
        PaletteColor::EightBit(4),
    ]);
    let styling = Styling {
        text_unselected: indexed,
        text_selected: declaration([
            PaletteColor::EightBit(15),
            PaletteColor::EightBit(8),
            PaletteColor::EightBit(1),
            PaletteColor::EightBit(2),
            PaletteColor::EightBit(3),
            PaletteColor::EightBit(4),
        ]),
        ribbon_unselected: declaration([
            PaletteColor::EightBit(0),
            PaletteColor::EightBit(7),
            PaletteColor::EightBit(1),
            PaletteColor::EightBit(15),
            PaletteColor::EightBit(4),
            PaletteColor::EightBit(5),
        ]),
        ribbon_selected: declaration([
            PaletteColor::EightBit(0),
            PaletteColor::EightBit(2),
            PaletteColor::EightBit(1),
            PaletteColor::EightBit(3),
            PaletteColor::EightBit(5),
            PaletteColor::EightBit(4),
        ]),
        frame_selected: indexed,
        frame_highlight: indexed,
        exit_code_success: indexed,
        exit_code_error: indexed,
    };

    let bar = BarTheme::from_styling(&styling);
    assert_eq!(bar.surface.background, PaletteColor::EightBit(0));
    assert_eq!(bar.active_tab.background, PaletteColor::EightBit(8));
    assert_eq!(bar.prefix.background, PaletteColor::EightBit(7));
    assert_eq!(bar.settings_prefix.background, PaletteColor::EightBit(2));
    assert!(theme::background(bar.surface.background).contains("[48;5;"));
    assert!(!theme::background(bar.surface.background).contains("[48;2;"));
}
