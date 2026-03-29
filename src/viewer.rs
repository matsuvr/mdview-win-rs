use std::{
    fs,
    path::{Path, PathBuf},
};

use gpui::prelude::*;
use gpui::{
    AnyElement, Context, ElementId, FocusHandle, FontWeight, IntoElement, Render,
    SharedString, StyledText, TextAlign, TextStyle, Window, div, img, px,
};
use pulldown_cmark::Alignment;
use thiserror::Error;

use crate::{
    assets::AppAssets,
    markdown::{
        Block, ListBlock, MathRender, MermaidRender, MarkdownDocument, RichText, Segment,
        TableBlock, parse_markdown,
    },
    math::hydrate_math_blocks,
    mermaid::hydrate_mermaid_blocks,
    theme::Theme,
};

// ---------------------------------------------------------------------------
// Page state
// ---------------------------------------------------------------------------

#[derive(Clone)]
enum PageState {
    Welcome,
    Loaded {
        requested_path: PathBuf,
        document: MarkdownDocument,
        math_prefix: String,
        mermaid_prefix: String,
    },
    Error {
        path: PathBuf,
        message: String,
    },
}

// ---------------------------------------------------------------------------
// MarkdownWindow
// ---------------------------------------------------------------------------

pub struct MarkdownWindow {
    focus_handle: FocusHandle,
    assets: AppAssets,
    theme: Theme,
    page: PageState,
}

// ---------------------------------------------------------------------------
// Load error
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
enum LoadError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("File is not valid UTF-8")]
    NotUtf8,
}

// ---------------------------------------------------------------------------
// Construction
// ---------------------------------------------------------------------------

impl MarkdownWindow {
    pub fn welcome(cx: &mut Context<Self>) -> Self {
        let assets = cx.try_global::<AppAssets>().cloned().unwrap_or_default();
        Self {
            focus_handle: cx.focus_handle(),
            assets,
            theme: Theme::default(),
            page: PageState::Welcome,
        }
    }

    pub fn from_path(
        requested_path: PathBuf,
        canonical_path: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) -> Self {
        let assets = cx.try_global::<AppAssets>().cloned().unwrap_or_default();
        let mut window = Self {
            focus_handle: cx.focus_handle(),
            assets,
            theme: Theme::default(),
            page: PageState::Welcome,
        };
        window.load_path(requested_path, canonical_path);
        window
    }

    pub fn reload_from_request(
        &mut self,
        requested_path: PathBuf,
        canonical_path: Option<PathBuf>,
        _cx: &mut Context<Self>,
    ) {
        self.load_path(requested_path, canonical_path);
    }

    pub fn sync_window(&mut self, window: &mut Window) {
        let title = match &self.page {
            PageState::Welcome => "mdview".to_string(),
            PageState::Loaded { document, .. } => {
                document.title.clone().unwrap_or_else(|| "mdview".to_string())
            }
            PageState::Error { path, .. } => {
                format!("Error — {}", path.display())
            }
        };
        window.set_window_title(&title);
    }

    pub fn focus_window(&self, window: &mut Window) {
        window.focus(&self.focus_handle);
    }

    fn load_path(&mut self, requested_path: PathBuf, canonical_path: Option<PathBuf>) {
        let source = match load_source(&requested_path) {
            Ok(s) => s,
            Err(err) => {
                self.page = PageState::Error {
                    path: requested_path,
                    message: err.to_string(),
                };
                return;
            }
        };

        let mut document = parse_markdown(&source);

        let canon = canonical_path.as_deref();
        let math_prefix = hydrate_math_blocks(&mut document, &requested_path, canon, &self.assets);
        let mermaid_prefix =
            hydrate_mermaid_blocks(&mut document, &requested_path, canon, &self.assets);

        self.page = PageState::Loaded {
            requested_path,
            document,
            math_prefix,
            mermaid_prefix,
        };
    }
}

// ---------------------------------------------------------------------------
// File loading
// ---------------------------------------------------------------------------

fn load_source(path: &Path) -> Result<String, LoadError> {
    let bytes = fs::read(path)?;
    String::from_utf8(bytes).map_err(|_| LoadError::NotUtf8)
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

impl Render for MarkdownWindow {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();

        let content: AnyElement = match &self.page {
            PageState::Welcome => render_welcome(&theme).into_any_element(),
            PageState::Loaded { document, .. } => {
                render_document(document, &theme).into_any_element()
            }
            PageState::Error { path, message } => {
                render_error(path, message, &theme).into_any_element()
            }
        };

        div()
            .id("markdown-scroll")
            .track_focus(&self.focus_handle)
            .bg(theme.background)
            .size_full()
            .overflow_y_scroll()
            .child(
                div()
                    .max_w(px(900.0))
                    .mx_auto()
                    .px(px(32.0))
                    .py(px(24.0))
                    .child(content),
            )
    }
}

// ---------------------------------------------------------------------------
// Page renderers
// ---------------------------------------------------------------------------

fn render_welcome(theme: &Theme) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(16.0))
        .child(
            div().child(StyledText::new("mdview").with_default_highlights(
                &theme.page_title_text_style(),
                [],
            )),
        )
        .child(div().child(StyledText::new(
            "ファイルをコマンドライン引数で指定してください。\n例: mdview.exe README.md",
        ).with_default_highlights(&theme.body_text_style(), [])))
}

fn render_error(path: &Path, message: &str, theme: &Theme) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(div().child(StyledText::new("ファイルを開けませんでした").with_default_highlights(
            &theme.error_title_text_style(),
            [],
        )))
        .child(div().child(
            StyledText::new(path.display().to_string()).with_default_highlights(&theme.error_text_style(), []),
        ))
        .child(div().child(
            StyledText::new(message.to_string()).with_default_highlights(&theme.body_text_style(), []),
        ))
}

fn render_document(document: &MarkdownDocument, theme: &Theme) -> impl IntoElement {
    let children: Vec<AnyElement> = document
        .blocks
        .iter()
        .map(|block| render_block(block, theme).into_any_element())
        .collect();

    div().flex().flex_col().gap(px(12.0)).children(children)
}

// ---------------------------------------------------------------------------
// Block renderers
// ---------------------------------------------------------------------------

fn render_block(block: &Block, theme: &Theme) -> impl IntoElement {
    match block {
        Block::Heading { level, content } => render_heading(*level, content, theme).into_any_element(),
        Block::Paragraph(rich) => render_paragraph(rich, theme).into_any_element(),
        Block::CodeBlock { language, code } => {
            render_code_block(language.as_deref(), code, theme).into_any_element()
        }
        Block::BlockQuote(children) => render_blockquote(children, theme).into_any_element(),
        Block::List(list) => render_list(list, theme).into_any_element(),
        Block::Rule => render_rule(theme).into_any_element(),
        Block::Table(table) => render_table(table, theme).into_any_element(),
        Block::Math(math) => render_math_block(math, theme).into_any_element(),
        Block::Mermaid(mermaid) => render_mermaid_block(mermaid, theme).into_any_element(),
    }
}

fn render_heading(level: u8, content: &RichText, theme: &Theme) -> impl IntoElement {
    let style = theme.heading_text_style(level);
    let text = content.plain_text();
    div()
        .mt(px(match level {
            1 => 24.0,
            2 => 20.0,
            _ => 16.0,
        }))
        .mb(px(8.0))
        .child(StyledText::new(text).with_default_highlights(&style, []))
}

fn render_paragraph(rich: &RichText, theme: &Theme) -> impl IntoElement {
    let style = theme.body_text_style();
    let children: Vec<AnyElement> = rich
        .segments
        .iter()
        .map(|seg| render_segment(seg, theme, &style))
        .collect();

    div()
        .flex()
        .flex_wrap()
        .items_baseline()
        .gap_x(px(0.0))
        .children(children)
}

fn render_segment(seg: &Segment, theme: &Theme, base_style: &TextStyle) -> AnyElement {
    match seg {
        Segment::Text(t) => {
            let style = theme.apply_inline_style(base_style.clone(), &t.style);
            StyledText::new(t.text.clone())
                .with_default_highlights(&style, [])
                .into_any_element()
        }
        Segment::InlineMath(math) => render_inline_math(math, theme).into_any_element(),
    }
}

fn render_inline_math(math: &crate::markdown::MathBlock, theme: &Theme) -> impl IntoElement {
    match &math.render {
        MathRender::Rendered { asset_path } => {
            img(SharedString::from(asset_path.clone()))
                .into_any_element()
        }
        MathRender::Pending => {
            let style = theme.mono_text_style();
            StyledText::new(format!("${}", math.latex))
                .with_default_highlights(&style, [])
                .into_any_element()
        }
        MathRender::Failed { message } => {
            let mut style = theme.mono_text_style();
            style.color = theme.error;
            StyledText::new(format!("${}$ ({})", math.latex, message))
                .with_default_highlights(&style, [])
                .into_any_element()
        }
    }
}

fn render_math_block(math: &crate::markdown::MathBlock, theme: &Theme) -> impl IntoElement {
    match &math.render {
        MathRender::Rendered { asset_path } => div()
            .my(px(16.0))
            .flex()
            .justify_center()
            .child(img(SharedString::from(asset_path.clone())))
            .into_any_element(),
        MathRender::Pending => {
            let style = theme.mono_text_style();
            div()
                .my(px(16.0))
                .px(px(16.0))
                .py(px(8.0))
                .bg(theme.code_background)
                .border_1()
                .border_color(theme.border)
                .child(StyledText::new(format!("$${}$$", math.latex)).with_default_highlights(&style, []))
                .into_any_element()
        }
        MathRender::Failed { message } => {
            let mut style = theme.mono_text_style();
            style.color = theme.error;
            div()
                .my(px(16.0))
                .child(StyledText::new(format!("[math error: {}]", message)).with_default_highlights(&style, []))
                .into_any_element()
        }
    }
}

fn render_mermaid_block(mermaid: &crate::markdown::MermaidBlock, theme: &Theme) -> impl IntoElement {
    match &mermaid.render {
        MermaidRender::Rendered { asset_path } => div()
            .my(px(16.0))
            .flex()
            .justify_center()
            .child(img(SharedString::from(asset_path.clone())))
            .into_any_element(),
        MermaidRender::Pending | MermaidRender::Failed { .. } => {
            let style = theme.mono_text_style();
            let msg = match &mermaid.render {
                MermaidRender::Failed { message } => format!("[mermaid error: {message}]"),
                _ => format!("```mermaid\n{}\n```", mermaid.code),
            };
            div()
                .my(px(16.0))
                .px(px(16.0))
                .py(px(8.0))
                .bg(theme.code_background)
                .border_1()
                .border_color(theme.border)
                .child(StyledText::new(msg).with_default_highlights(&style, []))
                .into_any_element()
        }
    }
}

fn render_code_block(language: Option<&str>, code: &str, theme: &Theme) -> impl IntoElement {
    let style = theme.mono_text_style();
    let header = language.map(|lang| {
        div()
            .px(px(12.0))
            .py(px(4.0))
            .bg(theme.border)
            .child(StyledText::new(lang.to_string()).with_default_highlights(&theme.mono_caption_text_style(), []))
            .into_any_element()
    });

    let mut container = div()
        .my(px(12.0))
        .bg(theme.code_background)
        .border_1()
        .border_color(theme.border)
        .flex()
        .flex_col();

    if let Some(h) = header {
        container = container.child(h);
    }

    container.child(
        div()
            .px(px(16.0))
            .py(px(12.0))
            
            .child(StyledText::new(code.to_string()).with_default_highlights(&style, [])),
    )
}

fn render_blockquote(children: &[Block], theme: &Theme) -> impl IntoElement {
    let child_elements: Vec<AnyElement> = children
        .iter()
        .map(|b| render_block(b, theme).into_any_element())
        .collect();

    div()
        .my(px(8.0))
        .pl(px(16.0))
        .border_l_4()
        .border_color(theme.quote_bar)
        .flex()
        .flex_col()
        .gap(px(8.0))
        .children(child_elements)
}

fn render_list(list: &ListBlock, theme: &Theme) -> impl IntoElement {
    let items: Vec<AnyElement> = list
        .items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let marker = if list.ordered {
                format!("{}.", list.start_index + i)
            } else {
                match item.task_state {
                    Some(true) => "☑".to_string(),
                    Some(false) => "☐".to_string(),
                    None => "•".to_string(),
                }
            };

            let marker_style = theme.body_text_style();
            let item_blocks: Vec<AnyElement> = item
                .blocks
                .iter()
                .map(|b| render_block(b, theme).into_any_element())
                .collect();

            div()
                .flex()
                .gap(px(8.0))
                .child(
                    div()
                        .w(px(24.0))
                        .flex_shrink_0()
                        .child(StyledText::new(marker).with_default_highlights(&marker_style, [])),
                )
                .child(div().flex().flex_col().gap(px(4.0)).children(item_blocks))
                .into_any_element()
        })
        .collect();

    div().my(px(8.0)).flex().flex_col().gap(px(4.0)).children(items)
}

fn render_rule(theme: &Theme) -> impl IntoElement {
    div()
        .my(px(16.0))
        .h(px(1.0))
        .bg(theme.border)
}

fn render_table(table: &TableBlock, theme: &Theme) -> impl IntoElement {
    let header_style = {
        let mut s = theme.body_text_style();
        s.font_weight = FontWeight::BOLD;
        s
    };
    let cell_style = theme.body_text_style();

    let header_row: Option<AnyElement> = table.header.as_ref().map(|cells| {
        let cols: Vec<AnyElement> = cells
            .iter()
            .enumerate()
            .map(|(i, cell)| {
                let align = table.alignments.get(i).copied();
                div()
                    .flex_1()
                    .px(px(12.0))
                    .py(px(8.0))
                    .bg(theme.header_background)
                    .border_1()
                    .border_color(theme.border)
                    .child(
                        StyledText::new(cell.plain_text())
                            .with_default_highlights(&header_style, []),
                    )
                    .into_any_element()
            })
            .collect();
        div().flex().children(cols).into_any_element()
    });

    let rows: Vec<AnyElement> = table
        .rows
        .iter()
        .enumerate()
        .map(|(row_idx, cells)| {
            let cols: Vec<AnyElement> = cells
                .iter()
                .enumerate()
                .map(|(i, cell)| {
                    div()
                        .flex_1()
                        .px(px(12.0))
                        .py(px(6.0))
                        .border_1()
                        .border_color(theme.border)
                        .child(
                            StyledText::new(cell.plain_text()).with_default_highlights(&cell_style, []),
                        )
                        .into_any_element()
                })
                .collect();
            div().flex().children(cols).into_any_element()
        })
        .collect();

    let mut table_div = div()
        .my(px(16.0))
        .border_1()
        .border_color(theme.border)
        .flex()
        .flex_col();

    if let Some(h) = header_row {
        table_div = table_div.child(h);
    }

    table_div.children(rows)
}
