//! Design tokens (docs/02 §14) plus the glass layers used by the Board (docs/12 §5).

/// Straight-alpha sRGB color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const fn rgb(hex: u32) -> Rgba {
        Rgba {
            r: (hex >> 16) as u8,
            g: (hex >> 8) as u8,
            b: hex as u8,
            a: 255,
        }
    }
    pub const fn hexa(hex: u32, a: u8) -> Rgba {
        let c = Rgba::rgb(hex);
        Rgba { a, ..c }
    }
    pub const WHITE: Rgba = Rgba::rgb(0xFFFFFF);
    pub const BLACK: Rgba = Rgba::rgb(0x000000);
    pub const CLEAR: Rgba = Rgba::hexa(0, 0);

    /// Multiply the alpha by `f` (0–1).
    pub fn fade(self, f: f32) -> Rgba {
        Rgba {
            a: (self.a as f32 * f.clamp(0.0, 1.0)).round() as u8,
            ..self
        }
    }
    pub fn lerp(self, o: Rgba, t: f32) -> Rgba {
        let t = t.clamp(0.0, 1.0);
        let m = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
        Rgba {
            r: m(self.r, o.r),
            g: m(self.g, o.g),
            b: m(self.b, o.b),
            a: m(self.a, o.a),
        }
    }
    /// Relative luminance (0–1), for picking text contrast.
    pub fn luma(self) -> f32 {
        (0.2126 * self.r as f32 + 0.7152 * self.g as f32 + 0.0722 * self.b as f32) / 255.0
    }
    pub fn to_skia(self) -> tiny_skia::Color {
        tiny_skia::Color::from_rgba8(self.r, self.g, self.b, self.a)
    }
}

/// File categories for tinted type glyphs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Image,
    Document,
    Audio,
    Video,
    Archive,
    Data,
    Font,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    pub dark: bool,
    /// Painted over the OS glass so text stays readable on any wallpaper.
    pub glass_tint: Rgba,
    /// Opaque window background when there is no OS glass (top, bottom of a gradient).
    pub solid_bg: (Rgba, Rgba),
    pub panel: Rgba,
    pub panel_stroke: Rgba,
    /// The 1 px brighter top edge that makes glass panels read as lit from above.
    pub panel_highlight: Rgba,
    pub card: Rgba,
    pub card_hover: Rgba,
    pub control: Rgba,
    pub control_hover: Rgba,
    pub control_pressed: Rgba,
    pub control_stroke: Rgba,
    pub divider: Rgba,
    pub text: Rgba,
    pub text2: Rgba,
    pub text3: Rgba,
    pub accent: Rgba,
    pub accent_hover: Rgba,
    pub accent_pressed: Rgba,
    pub on_accent: Rgba,
    /// Accent wash behind selected cards and rows.
    pub accent_wash: Rgba,
    pub success: Rgba,
    pub warning: Rgba,
    pub error: Rgba,
    pub shadow: Rgba,
    pub close_hover: Rgba,
}

impl Theme {
    pub fn light() -> Theme {
        Theme {
            dark: false,
            glass_tint: Rgba::hexa(0xF7F5F3, 110),
            solid_bg: (Rgba::rgb(0xF4F1EE), Rgba::rgb(0xFBF8F5)),
            panel: Rgba::hexa(0xFFFFFF, 150),
            panel_stroke: Rgba::hexa(0x000000, 16),
            panel_highlight: Rgba::hexa(0xFFFFFF, 200),
            card: Rgba::hexa(0xFFFFFF, 175),
            card_hover: Rgba::hexa(0xFFFFFF, 235),
            control: Rgba::hexa(0xFFFFFF, 200),
            control_hover: Rgba::hexa(0xFFFFFF, 250),
            control_pressed: Rgba::hexa(0xF2F2F2, 230),
            control_stroke: Rgba::hexa(0x000000, 26),
            divider: Rgba::hexa(0x000000, 18),
            text: Rgba::hexa(0x000000, 230),
            text2: Rgba::hexa(0x000000, 158),
            text3: Rgba::hexa(0x000000, 92),
            accent: Rgba::rgb(0xE8772E),
            accent_hover: Rgba::rgb(0xD96A22),
            accent_pressed: Rgba::rgb(0xC45F1E),
            on_accent: Rgba::rgb(0x1F1006),
            accent_wash: Rgba::hexa(0xE8772E, 34),
            success: Rgba::rgb(0x0F7B0F),
            warning: Rgba::rgb(0x9D5D00),
            error: Rgba::rgb(0xC42B1C),
            shadow: Rgba::hexa(0x000000, 40),
            close_hover: Rgba::rgb(0xC42B1C),
        }
    }

    pub fn dark() -> Theme {
        Theme {
            dark: true,
            glass_tint: Rgba::hexa(0x18181B, 96),
            solid_bg: (Rgba::rgb(0x1C1B1D), Rgba::rgb(0x242120)),
            panel: Rgba::hexa(0xFFFFFF, 12),
            panel_stroke: Rgba::hexa(0xFFFFFF, 20),
            panel_highlight: Rgba::hexa(0xFFFFFF, 26),
            card: Rgba::hexa(0xFFFFFF, 15),
            card_hover: Rgba::hexa(0xFFFFFF, 26),
            control: Rgba::hexa(0xFFFFFF, 20),
            control_hover: Rgba::hexa(0xFFFFFF, 32),
            control_pressed: Rgba::hexa(0xFFFFFF, 12),
            control_stroke: Rgba::hexa(0xFFFFFF, 22),
            divider: Rgba::hexa(0xFFFFFF, 18),
            text: Rgba::rgb(0xFFFFFF),
            text2: Rgba::hexa(0xFFFFFF, 200),
            text3: Rgba::hexa(0xFFFFFF, 110),
            accent: Rgba::rgb(0xFF9A57),
            accent_hover: Rgba::rgb(0xFFAD75),
            accent_pressed: Rgba::rgb(0xFFC093),
            on_accent: Rgba::rgb(0x1F1206),
            accent_wash: Rgba::hexa(0xFF9A57, 40),
            success: Rgba::rgb(0x6CCB5F),
            warning: Rgba::rgb(0xFCE100),
            error: Rgba::rgb(0xFF99A4),
            shadow: Rgba::hexa(0x000000, 90),
            close_hover: Rgba::rgb(0xC42B1C),
        }
    }

    pub fn for_dark(dark: bool) -> Theme {
        if dark { Theme::dark() } else { Theme::light() }
    }

    /// Tint for a file-type glyph.
    pub fn kind(&self, k: Kind) -> Rgba {
        let (l, d) = match k {
            Kind::Image => (0x2E8B57, 0x5FD08A),
            Kind::Document => (0x2F6FDB, 0x7AA7FF),
            Kind::Audio => (0xB4469C, 0xE58BD2),
            Kind::Video => (0xC2410C, 0xFF8A5B),
            Kind::Archive => (0x8A6D1E, 0xE0C064),
            Kind::Data => (0x0E8A84, 0x3CC9C0),
            Kind::Font => (0x5B5FD6, 0x9094F7),
            Kind::Other => (0x6B7280, 0xA1A7B3),
        };
        Rgba::rgb(if self.dark { d } else { l })
    }

    /// Tint for the `k`-th of several groups (split parts, series), cycling after eight.
    pub fn series(&self, k: usize) -> Rgba {
        const L: [u32; 8] = [
            0x2F6FDB, 0xC2410C, 0x2E8B57, 0xB4469C, 0x8A6D1E, 0x0E8A84, 0x5B5FD6, 0xB91C1C,
        ];
        const D: [u32; 8] = [
            0x7AA7FF, 0xFF8A5B, 0x5FD08A, 0xE58BD2, 0xE0C064, 0x3CC9C0, 0x9094F7, 0xF87171,
        ];
        Rgba::rgb(if self.dark { D[k % 8] } else { L[k % 8] })
    }
}

/// Spacing and size tokens (logical px, 4 px grid).
pub mod size {
    pub const XS: f32 = 4.0;
    pub const S: f32 = 8.0;
    pub const M: f32 = 12.0;
    pub const L: f32 = 16.0;
    pub const XL: f32 = 24.0;
    pub const XXL: f32 = 32.0;
    pub const CONTROL_H: f32 = 32.0;
    pub const R_CONTROL: f32 = 6.0;
    pub const R_CARD: f32 = 10.0;
    pub const R_PANEL: f32 = 12.0;
    pub const CAPTION: f32 = 12.0;
    pub const BODY: f32 = 13.0;
    pub const BODY_L: f32 = 14.0;
    pub const SUBTITLE: f32 = 18.0;
    pub const TITLE: f32 = 26.0;
}
