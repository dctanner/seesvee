use gpui::{App, Global, Hsla, WindowAppearance, rgb, rgba};

#[derive(Clone, Copy)]
pub struct Theme {
    pub bg: Hsla,
    pub zebra: Hsla,
    pub header_bg: Hsla,
    pub header_text: Hsla,
    pub grid_line: Hsla,
    pub border: Hsla,
    pub text: Hsla,
    pub text_muted: Hsla,
    pub text_faint: Hsla,
    pub accent: Hsla,
    pub accent_text: Hsla,
    pub selection: Hsla,
    pub selection_header: Hsla,
    pub error: Hsla,
    pub warning_bg: Hsla,
    pub warning_text: Hsla,
    pub surface: Hsla,
    pub hover: Hsla,
    pub scrollbar: Hsla,
    pub scrollbar_active: Hsla,
    pub input_bg: Hsla,
}

impl Global for Theme {}

impl Theme {
    pub fn light() -> Self {
        Self {
            bg: rgb(0xFFFFFF).into(),
            zebra: rgb(0xFAFAFB).into(),
            header_bg: rgb(0xF6F6F7).into(),
            header_text: rgb(0x4A4A50).into(),
            grid_line: rgb(0xEDEDEF).into(),
            border: rgb(0xE1E1E4).into(),
            text: rgb(0x1D1D1F).into(),
            text_muted: rgb(0x6E6E73).into(),
            text_faint: rgb(0xA1A1A6).into(),
            accent: rgb(0x0A6CFF).into(),
            accent_text: rgb(0xFFFFFF).into(),
            selection: rgba(0x0A6CFF14).into(),
            selection_header: rgba(0x0A6CFF1F).into(),
            error: rgb(0xD93036).into(),
            warning_bg: rgb(0xFFF6E5).into(),
            warning_text: rgb(0x8A5300).into(),
            surface: rgb(0xFFFFFF).into(),
            hover: rgba(0x0000000D).into(),
            scrollbar: rgba(0x00000033).into(),
            scrollbar_active: rgba(0x00000066).into(),
            input_bg: rgb(0xFFFFFF).into(),
        }
    }

    pub fn dark() -> Self {
        Self {
            bg: rgb(0x1C1C1E).into(),
            zebra: rgb(0x202022).into(),
            header_bg: rgb(0x252527).into(),
            header_text: rgb(0xC7C7CC).into(),
            grid_line: rgb(0x2C2C2F).into(),
            border: rgb(0x38383B).into(),
            text: rgb(0xE9E9EB).into(),
            text_muted: rgb(0x98989F).into(),
            text_faint: rgb(0x636366).into(),
            accent: rgb(0x3D8BFF).into(),
            accent_text: rgb(0xFFFFFF).into(),
            selection: rgba(0x3D8BFF24).into(),
            selection_header: rgba(0x3D8BFF33).into(),
            error: rgb(0xFF5F57).into(),
            warning_bg: rgb(0x3A2E14).into(),
            warning_text: rgb(0xF5C26B).into(),
            surface: rgb(0x2A2A2D).into(),
            hover: rgba(0xFFFFFF14).into(),
            scrollbar: rgba(0xFFFFFF33).into(),
            scrollbar_active: rgba(0xFFFFFF66).into(),
            input_bg: rgb(0x1C1C1E).into(),
        }
    }

    pub fn for_appearance(appearance: WindowAppearance) -> Self {
        match appearance {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self::dark(),
            WindowAppearance::Light | WindowAppearance::VibrantLight => Self::light(),
        }
    }

    pub fn get(cx: &App) -> Theme {
        *cx.global::<Theme>()
    }
}
