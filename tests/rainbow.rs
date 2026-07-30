#![allow(dead_code)]

#[path = "../src/rainbow.rs"]
mod rainbow;

#[test]
fn wheel_hits_all_six_primary_boundaries_and_wraps() {
    assert_eq!(rainbow::wheel_rgb(0), (255, 0, 0));
    assert_eq!(rainbow::wheel_rgb(256), (255, 255, 0));
    assert_eq!(rainbow::wheel_rgb(512), (0, 255, 0));
    assert_eq!(rainbow::wheel_rgb(768), (0, 255, 255));
    assert_eq!(rainbow::wheel_rgb(1024), (0, 0, 255));
    assert_eq!(rainbow::wheel_rgb(1280), (255, 0, 255));
    assert_eq!(rainbow::wheel_rgb(1536), (255, 0, 0));
}

#[test]
fn time_and_character_position_shift_the_color() {
    let initial = rainbow::rainbow_rgb(0, 0, 0, false);
    assert_ne!(rainbow::rainbow_rgb(250, 0, 0, false), initial);
    assert_ne!(rainbow::rainbow_rgb(0, 1, 0, false), initial);
    assert_eq!(rainbow::rainbow_rgb(12_000, 0, 0, false), initial);
}

#[test]
fn inactive_colors_are_dimmed() {
    let bright = rainbow::rainbow_rgb(0, 0, 0, false);
    let dimmed = rainbow::rainbow_rgb(0, 0, 0, true);

    assert_eq!(bright, (255, 0, 0));
    assert_eq!(dimmed, (204, 0, 0));
}

#[test]
fn contrast_guard_lifts_dark_hues_on_gruvbox_dark() {
    let background = (80, 73, 69);
    let theme_foreground = (251, 241, 199);
    let adjusted = rainbow::ensure_contrast((0, 0, 255), background, theme_foreground);

    assert_ne!(adjusted, (0, 0, 255));
    assert!(rainbow::contrast_ratio(adjusted, background) >= 4.5);
}

#[test]
fn contrast_guard_preserves_hues_that_are_already_readable() {
    let color = (255, 255, 0);
    let background = (60, 56, 54);
    let theme_foreground = (251, 241, 199);

    assert_eq!(
        rainbow::ensure_contrast(color, background, theme_foreground),
        color
    );
}

#[test]
fn contrast_guard_darkens_hues_for_light_theme_backgrounds() {
    let background = (255, 255, 255);
    let theme_foreground = (30, 30, 30);
    let adjusted = rainbow::ensure_contrast((255, 255, 0), background, theme_foreground);

    assert_ne!(adjusted, (255, 255, 0));
    assert!(rainbow::contrast_ratio(adjusted, background) >= 4.5);
}

#[test]
fn writer_preserves_multibyte_text() {
    let mut output = String::new();
    rainbow::write_rainbow(
        &mut output,
        "코드빛",
        0,
        0,
        false,
        Some((80, 73, 69)),
        Some((251, 241, 199)),
    )
    .unwrap();

    assert!(output.contains('코'));
    assert!(output.contains('드'));
    assert!(output.contains('빛'));
    assert_eq!(output.matches("\x1b[38;2;").count(), 3);
}
