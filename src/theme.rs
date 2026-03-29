use gpui::{
    FontStyle, FontWeight, Hsla, StrikethroughStyle, TextAlign, TextStyle, UnderlineStyle,
    WhiteSpace, px, rgb,
};

use crate::markdown::InlineStyle;

const BODY_FONT: &str = "Segoe UI";
pub(crate) const MONO_FONT: &str = "Consolas";

#[derive(Clone, Debug)]
pub struct Theme {
    pub background: Hsla,
    pub header_background: Hsla,
    pub text: Hsla,
    pub muted_text: Hsla,
    pub border: Hsla,
    pub code_background: Hsla,
    pub inline_code_background: Hsla,
    pub quote_bar: Hsla,
    pub link: Hsla,
    pub error: Hsla,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            background: rgb(0xffffff).into(),
            header_background: rgb(0xffffff).into(),
            text: rgb(0x1f2328).into(),
            muted_text: rgb(0x57606a).into(),
            border: rgb(0xd0d7de).into(),
            code_background: rgb(0xffffff).into(),
            inline_code_background: rgb(0xeaeef2).into(),
            quote_bar: rgb(0x9ba7b4).into(),
            link: rgb(0x0969da).into(),
            error: rgb(0xcf222e).into(),
        }
    }
}

impl Theme {
    pub fn body_text_style(&self) -> TextStyle {
        Self::text_style(
            BODY_FONT,
            16.0,
            24.0,
            self.text,
            FontWeight::NORMAL,
            FontStyle::Normal,
        )
    }

    pub fn caption_text_style(&self) -> TextStyle {
        Self::text_style(
            BODY_FONT,
            13.0,
            18.0,
            self.muted_text,
            FontWeight::NORMAL,
            FontStyle::Normal,
        )
    }

    pub fn header_title_text_style(&self) -> TextStyle {
        Self::text_style(
            BODY_FONT,
            16.0,
            22.0,
            self.text,
            FontWeight::SEMIBOLD,
            FontStyle::Normal,
        )
    }

    pub fn page_title_text_style(&self) -> TextStyle {
        Self::text_style(
            BODY_FONT,
            26.0,
            34.0,
            self.text,
            FontWeight::BOLD,
            FontStyle::Normal,
        )
    }

    pub fn error_title_text_style(&self) -> TextStyle {
        Self::text_style(
            BODY_FONT,
            24.0,
            32.0,
            self.error,
            FontWeight::BOLD,
            FontStyle::Normal,
        )
    }

    pub fn error_text_style(&self) -> TextStyle {
        Self::text_style(
            BODY_FONT,
            15.0,
            22.0,
            self.error,
            FontWeight::SEMIBOLD,
            FontStyle::Normal,
        )
    }

    pub fn heading_text_style(&self, level: u8) -> TextStyle {
        match level {
            1 => Self::text_style(
                BODY_FONT,
                32.0,
                40.0,
                self.text,
                FontWeight::BOLD,
                FontStyle::Normal,
            ),
            2 => Self::text_style(
                BODY_FONT,
                28.0,
                36.0,
                self.text,
                FontWeight::BOLD,
                FontStyle::Normal,
            ),
            3 => Self::text_style(
                BODY_FONT,
                24.0,
                32.0,
                self.text,
                FontWeight::SEMIBOLD,
                FontStyle::Normal,
            ),
            4 => Self::text_style(
                BODY_FONT,
                20.0,
                28.0,
                self.text,
                FontWeight::SEMIBOLD,
                FontStyle::Normal,
            ),
            5 => Self::text_style(
                BODY_FONT,
                18.0,
                26.0,
                self.text,
                FontWeight::SEMIBOLD,
                FontStyle::Normal,
            ),
            _ => Self::text_style(
                BODY_FONT,
                16.0,
                24.0,
                self.text,
                FontWeight::SEMIBOLD,
                FontStyle::Normal,
            ),
        }
    }

    pub fn mono_text_style(&self) -> TextStyle {
        Self::text_style(
            MONO_FONT,
            14.0,
            20.0,
            self.text,
            FontWeight::NORMAL,
            FontStyle::Normal,
        )
    }

    pub fn mono_caption_text_style(&self) -> TextStyle {
        Self::text_style(
            MONO_FONT,
            12.0,
            18.0,
            self.muted_text,
            FontWeight::NORMAL,
            FontStyle::Normal,
        )
    }

    pub fn apply_inline_style(&self, mut base: TextStyle, span: &InlineStyle) -> TextStyle {
        if span.bold {
            base.font_weight = FontWeight::BOLD;
        }

        if span.italic {
            base.font_style = FontStyle::Italic;
        }

        if span.code {
            base.font_family = MONO_FONT.into();
            base.background_color = Some(self.inline_code_background);
        }

        if span.link_target.is_some() {
            base.color = self.link;
            base.underline = Some(UnderlineStyle {
                thickness: px(1.0),
                color: Some(self.link),
                wavy: false,
            });
        }

        if span.strike {
            base.strikethrough = Some(StrikethroughStyle {
                thickness: px(1.0),
                color: Some(base.color),
            });
        }

        base
    }

    fn text_style(
        font_family: &'static str,
        font_size: f32,
        line_height: f32,
        color: Hsla,
        font_weight: FontWeight,
        font_style: FontStyle,
    ) -> TextStyle {
        TextStyle {
            color,
            font_family: font_family.into(),
            font_fallbacks: None,
            font_features: Default::default(),
            font_size: px(font_size).into(),
            line_height: px(line_height).into(),
            font_weight,
            font_style,
            background_color: None,
            underline: None,
            strikethrough: None,
            white_space: WhiteSpace::Normal,
            text_overflow: None,
            text_align: TextAlign::Left,
            line_clamp: None,
        }
    }
}
