use std::fmt::{self, Write};

pub const ANIMATION_TICK_SECONDS: f64 = 0.25;

const WHEEL_STEPS: u64 = 1536;
const FRAME_MS: u64 = 250;
const FRAME_STEP: u64 = 32;
const CHARACTER_STEP: u64 = 96;
const TAB_STEP: u64 = 173;
const MIN_CONTRAST_RATIO: f64 = 4.5;

pub type Rgb = (u8, u8, u8);

/// Return a color from a six-segment RGB wheel:
/// red → yellow → green → cyan → blue → magenta → red.
pub fn wheel_rgb(step: u16) -> Rgb {
    let step = u32::from(step) % WHEEL_STEPS as u32;
    let segment = step / 256;
    let rising = (step % 256) as u8;
    let falling = 255 - rising;

    match segment {
        0 => (255, rising, 0),
        1 => (falling, 255, 0),
        2 => (0, 255, rising),
        3 => (0, falling, 255),
        4 => (rising, 0, 255),
        _ => (255, 0, falling),
    }
}

pub fn rainbow_rgb(now_ms: u64, character_index: usize, tab_index: usize, dimmed: bool) -> Rgb {
    let frame = now_ms / FRAME_MS;
    let step = (frame * FRAME_STEP
        + character_index as u64 * CHARACTER_STEP
        + tab_index as u64 * TAB_STEP)
        % WHEEL_STEPS;
    let (r, g, b) = wheel_rgb(step as u16);

    if dimmed {
        (
            ((u16::from(r) * 4) / 5) as u8,
            ((u16::from(g) * 4) / 5) as u8,
            ((u16::from(b) * 4) / 5) as u8,
        )
    } else {
        (r, g, b)
    }
}

fn linear_channel(channel: u8) -> f64 {
    let value = f64::from(channel) / 255.0;
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn relative_luminance(color: Rgb) -> f64 {
    0.2126 * linear_channel(color.0)
        + 0.7152 * linear_channel(color.1)
        + 0.0722 * linear_channel(color.2)
}

pub fn contrast_ratio(first: Rgb, second: Rgb) -> f64 {
    let first = relative_luminance(first);
    let second = relative_luminance(second);
    let (lighter, darker) = if first >= second {
        (first, second)
    } else {
        (second, first)
    };
    (lighter + 0.05) / (darker + 0.05)
}

fn mix(from: Rgb, toward: Rgb, amount: f64) -> Rgb {
    let channel = |from: u8, toward: u8| {
        (f64::from(from) + (f64::from(toward) - f64::from(from)) * amount)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    (
        channel(from.0, toward.0),
        channel(from.1, toward.1),
        channel(from.2, toward.2),
    )
}

/// Preserve an animated hue when it is readable. Otherwise, make the
/// smallest blend toward the theme's paired tab foreground that reaches
/// normal-text contrast.
pub fn ensure_contrast(color: Rgb, background: Rgb, theme_foreground: Rgb) -> Rgb {
    let color_contrast = contrast_ratio(color, background);
    if color_contrast >= MIN_CONTRAST_RATIO {
        return color;
    }

    let foreground_contrast = contrast_ratio(theme_foreground, background);
    if foreground_contrast < MIN_CONTRAST_RATIO {
        return if foreground_contrast > color_contrast {
            theme_foreground
        } else {
            color
        };
    }

    let mut low = 0.0;
    let mut high = 1.0;
    for _ in 0..12 {
        let midpoint = (low + high) / 2.0;
        if contrast_ratio(mix(color, theme_foreground, midpoint), background) >= MIN_CONTRAST_RATIO
        {
            high = midpoint;
        } else {
            low = midpoint;
        }
    }
    mix(color, theme_foreground, high)
}

pub fn write_rainbow(
    buf: &mut String,
    text: &str,
    now_ms: u64,
    tab_index: usize,
    dimmed: bool,
    background: Option<Rgb>,
    theme_foreground: Option<Rgb>,
) -> fmt::Result {
    for (character_index, character) in text.chars().enumerate() {
        let color = rainbow_rgb(now_ms, character_index, tab_index, dimmed);
        let (r, g, b) = match (background, theme_foreground) {
            (Some(background), Some(theme_foreground)) => {
                ensure_contrast(color, background, theme_foreground)
            }
            _ => color,
        };
        write!(buf, "\x1b[38;2;{r};{g};{b}m{character}")?;
    }
    Ok(())
}
