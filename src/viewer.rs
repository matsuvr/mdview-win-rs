use std::{
    fs,
    path::{Path, PathBuf},
};

use gpui::prelude::*;
use gpui::{
    AnyElement, BorrowAppContext, Context, ElementId, FocusHandle, FontWeight, IntoElement, Render,
    StyledText, TextAlign, TextStyle, Window, div, px, svg,
};
use pulldown_cmark::Alignment;
use thiserror::Error;

use crate::{
    assets::AppAssets,
    markdown::{
        Block, ListBlock, MarkdownDocument, MermaidBlock, MermaidRender, RichText, TableBlock,
        parse_markdown,
    },
    mermaid::hydrate_mermaid_blocks,
    theme::Theme,
};

pub struct MarkdownWindow {
    focus_handle: FocusHandle,
    assets: AppAssets,
    theme: Theme,
    page: PageState,
}

#[derive(Clone, Debug)]
enum PageState {
    Welcome,
    Document(DocumentPage),
    Error(ErrorPage),
}

#[derive(Clone, Debug)]
struct DocumentPage {
    requested_path: PathBuf,
    canonical_path: Option<PathBuf>,
    asset_prefix: String,
    document: MarkdownDocument,
}

#[derive(Clone, Debug)]
struct ErrorPage {
    requested_path: Option<PathBuf>,
    message: String,
}

#[derive(Debug, Error)]
enum LoadError {
    #[error("failed to read the file: {0}")]
    Io(#[from] std::io::Error),
    #[error("the file is not valid UTF-8")]
    InvalidUtf8(#[from] std::string::FromUtf8Error),
}

impl MarkdownWindow {
    pub fn welcome(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            assets: clone_app_assets(cx),
            theme: Theme::default(),
            page: PageState::Welcome,
        }
    }

    pub fn from_path(
        requested_path: PathBuf,
        canonical_path: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) -> Self {
        let assets = clone_app_assets(cx);
        let theme = Theme::default();
        let page = load_page(requested_path, canonical_path, &assets);

        Self {
            focus_handle: cx.focus_handle(),
            assets,
            theme,
            page,
        }
    }

    pub fn reload_from_request(
        &mut self,
        requested_path: PathBuf,
        canonical_path: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        self.clear_page_assets();
        self.page = load_page(requested_path, canonical_path, &self.assets);
        cx.notify();
    }

    pub fn sync_window(&self, window: &mut Window) {
        let title = self.window_title();
        window.set_window_title(&title);
    }

    pub fn focus_window(&self, window: &mut Window) {
        window.activate_window();
        window.focus(&self.focus_handle);
    }

    fn clear_page_assets(&self) {
        if let PageState::Document(page) = &self.page {
            self.assets.remove_prefix(&page.asset_prefix);
        }
    }

    fn window_title(&self) -> String {
        match &self.page {
            PageState::Welcome => "mdview".to_string(),
            PageState::Error(error) => {
                let label = error
                    .requested_path
                    .as_deref()
                    .map(display_name)
                    .unwrap_or_else(|| "Open error".to_string());
                format!("{label} · mdview")
            }
            PageState::Document(page) => {
                let label = display_name(&page.requested_path);
                format!("{label} · mdview")
            }
        }
    }

    fn render_document_page(&self, page: &DocumentPage) -> AnyElement {
        let header_title = display_name(&page.requested_path);
        let header_path = page
            .canonical_path
            .as_deref()
            .map(path_to_lossy_string)
            .unwrap_or_else(|| path_to_lossy_string(&page.requested_path));

        let content = div().w_full().flex().flex_col().gap_4().children(
            page.document
                .blocks
                .iter()
                .map(|block| self.render_block(block)),
        );

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(self.theme.background)
            .track_focus(&self.focus_handle)
            .tab_stop(true)
            .child(self.render_header(&header_title, &header_path, page.document.title.as_deref()))
            .child(div().w_full().h(px(1.0)).bg(self.theme.border))
            .child(
                div()
                    .id(ElementId::Name(
                        format!("document-scroll:{}", page.asset_prefix).into(),
                    ))
                    .flex_1()
                    .overflow_scroll()
                    .px_6()
                    .py_5()
                    .child(content),
            )
            .into_any_element()
    }

    fn render_welcome_page(&self) -> AnyElement {
        let paragraphs = [
            "超シンプルな読み取り専用マークダウンビューワーです。編集機能、タブ、ツールバー、ファイルツリーはありません。",
            "起動例: mdview.exe README.md docs/spec.md",
            "複数ファイルはそのまま複数ウインドウで開きます。同じ実体パスのファイルが再度要求された場合は、既存のウインドウへフォーカスを移します。",
            "```mermaid``` フェンスは pure Rust の mermaid-rs-renderer で SVG 化して表示します。",
        ];

        let body = div()
            .w_full()
            .flex()
            .flex_col()
            .gap_4()
            .child(self.render_plain_block("mdview", self.theme.page_title_text_style()))
            .children(
                paragraphs.into_iter().map(|paragraph| {
                    self.render_plain_block(paragraph, self.theme.body_text_style())
                }),
            )
            .child(self.render_plain_block(
                "設計方針: read-only / one-window-per-file / Rust + GPUI / no unsafe in this crate",
                self.theme.caption_text_style(),
            ));

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(self.theme.background)
            .track_focus(&self.focus_handle)
            .tab_stop(true)
            .child(self.render_header("mdview", "ready", None))
            .child(div().w_full().h(px(1.0)).bg(self.theme.border))
            .child(
                div()
                    .id("welcome-scroll")
                    .flex_1()
                    .overflow_scroll()
                    .px_6()
                    .py_5()
                    .child(body),
            )
            .into_any_element()
    }

    fn render_error_page(&self, page: &ErrorPage) -> AnyElement {
        let header_title = page
            .requested_path
            .as_deref()
            .map(display_name)
            .unwrap_or_else(|| "Open error".to_string());
        let header_path = page
            .requested_path
            .as_deref()
            .map(path_to_lossy_string)
            .unwrap_or_else(|| "path unavailable".to_string());

        let body = div()
            .w_full()
            .flex()
            .flex_col()
            .gap_4()
            .child(self.render_plain_block(
                "ファイルを開けませんでした",
                self.theme.error_title_text_style(),
            ))
            .child(self.render_plain_block(&page.message, self.theme.body_text_style()));

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(self.theme.background)
            .track_focus(&self.focus_handle)
            .tab_stop(true)
            .child(self.render_header(&header_title, &header_path, None))
            .child(div().w_full().h(px(1.0)).bg(self.theme.border))
            .child(
                div()
                    .id("error-scroll")
                    .flex_1()
                    .overflow_scroll()
                    .px_6()
                    .py_5()
                    .child(body),
            )
            .into_any_element()
    }

    fn render_header(
        &self,
        title: &str,
        subtitle: &str,
        document_title: Option<&str>,
    ) -> AnyElement {
        let mut column = div()
            .w_full()
            .flex()
            .flex_col()
            .gap_1()
            .child(self.plain_styled_text(title, self.theme.header_title_text_style()));

        if let Some(document_title) = document_title {
            if !document_title.is_empty() && document_title != title {
                column = column
                    .child(self.plain_styled_text(document_title, self.theme.caption_text_style()));
            }
        }

        column = column.child(self.plain_styled_text(subtitle, self.theme.caption_text_style()));

        div()
            .w_full()
            .bg(self.theme.header_background)
            .px_6()
            .py_4()
            .child(column)
            .into_any_element()
    }

    fn render_block(&self, block: &Block) -> AnyElement {
        match block {
            Block::Heading { level, content } => {
                self.render_rich_block(content, self.theme.heading_text_style(*level))
            }
            Block::Paragraph(content) => {
                self.render_rich_block(content, self.theme.body_text_style())
            }
            Block::CodeBlock { language, code } => {
                self.render_code_block(language.as_deref(), code)
            }
            Block::Mermaid(block) => self.render_mermaid_block(block),
            Block::BlockQuote(blocks) => {
                let content = div()
                    .flex_1()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .children(blocks.iter().map(|block| self.render_block(block)));

                div()
                    .w_full()
                    .flex()
                    .gap_3()
                    .child(div().w(px(3.0)).bg(self.theme.quote_bar))
                    .child(content)
                    .into_any_element()
            }
            Block::List(list) => self.render_list(list),
            Block::Rule => div()
                .w_full()
                .h(px(1.0))
                .bg(self.theme.border)
                .into_any_element(),
            Block::Table(table) => self.render_table(table),
        }
    }

    fn render_list(&self, list: &ListBlock) -> AnyElement {
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_2()
            .children(list.items.iter().enumerate().map(|(index, item)| {
                let marker = if let Some(checked) = item.task_state {
                    if checked {
                        "[x]".to_string()
                    } else {
                        "[ ]".to_string()
                    }
                } else if list.ordered {
                    format!("{}.", list.start_index + index)
                } else {
                    "•".to_string()
                };

                let child_blocks = div()
                    .flex_1()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .children(item.blocks.iter().map(|block| self.render_block(block)));

                div()
                    .w_full()
                    .flex()
                    .gap_3()
                    .child(
                        div()
                            .w(px(36.0))
                            .child(self.plain_styled_text(&marker, self.theme.body_text_style())),
                    )
                    .child(child_blocks)
                    .into_any_element()
            }))
            .into_any_element()
    }

    fn render_mermaid_block(&self, block: &MermaidBlock) -> AnyElement {
        let mut content = div()
            .w_full()
            .flex()
            .flex_col()
            .gap_2()
            .child(self.render_plain_block("mermaid", self.theme.mono_caption_text_style()));

        match &block.render {
            MermaidRender::Rendered { asset_path } => {
                content = content.child(div().w_full().child(svg().path(asset_path.clone())));
            }
            MermaidRender::Failed { message } => {
                content = content
                    .child(self.render_plain_block(
                        "Mermaid のレンダリングに失敗しました。",
                        self.theme.error_text_style(),
                    ))
                    .child(self.render_plain_block(message, self.theme.caption_text_style()))
                    .child(self.render_code_block(Some("mermaid"), &block.code));
            }
            MermaidRender::Pending => {
                content = content.child(self.render_code_block(Some("mermaid"), &block.code));
            }
        }

        div()
            .w_full()
            .bg(self.theme.code_background)
            .p_3()
            .child(content)
            .into_any_element()
    }

    fn render_table(&self, table: &TableBlock) -> AnyElement {
        let column_widths = table_column_widths(table);
        if column_widths.is_empty() {
            return div().w_full().into_any_element();
        }

        let total_width: f32 = column_widths.iter().sum();
        let mut content = div()
            .flex()
            .flex_col()
            .w(px(total_width))
            .border_1()
            .border_color(self.theme.border)
            .rounded_sm()
            .overflow_hidden();

        let body_row_count = table.rows.len();

        if let Some(header) = table.header.as_deref() {
            content = content.child(self.render_table_row(
                header,
                &column_widths,
                &table.alignments,
                true,
                0,
                body_row_count > 0,
            ));
        }

        for (row_index, row) in table.rows.iter().enumerate() {
            content = content.child(self.render_table_row(
                row,
                &column_widths,
                &table.alignments,
                false,
                row_index,
                row_index + 1 < body_row_count,
            ));
        }

        content.into_any_element()
    }

    fn render_table_row(
        &self,
        row: &[RichText],
        column_widths: &[f32],
        alignments: &[Alignment],
        is_header: bool,
        row_index: usize,
        draw_bottom_border: bool,
    ) -> AnyElement {
        let mut row_element = div()
            .flex()
            .flex_row()
            .w(px(column_widths.iter().sum()))
            .bg(if is_header {
                self.theme.background
            } else if row_index.is_multiple_of(2) {
                self.theme.code_background
            } else {
                self.theme.background
            });

        if draw_bottom_border {
            row_element = row_element.border_b_1().border_color(self.theme.border);
        }

        row_element = row_element.children(column_widths.iter().enumerate().map(
            |(column_index, width)| {
                self.render_table_cell(
                    row.get(column_index),
                    *width,
                    alignments
                        .get(column_index)
                        .copied()
                        .unwrap_or(Alignment::None),
                    is_header,
                    column_index + 1 < column_widths.len(),
                )
            },
        ));

        row_element.into_any_element()
    }

    fn render_table_cell(
        &self,
        cell: Option<&RichText>,
        width: f32,
        alignment: Alignment,
        is_header: bool,
        draw_right_border: bool,
    ) -> AnyElement {
        let mut style = self.theme.body_text_style();
        style.text_align = table_text_align(alignment);
        if is_header {
            style.font_weight = FontWeight::SEMIBOLD;
        }

        let mut cell_element = div()
            .w(px(width))
            .flex_shrink_0()
            .px_3()
            .py_2()
            .child(self.render_rich_block(cell.unwrap_or(&RichText::default()), style));

        if draw_right_border {
            cell_element = cell_element.border_r_1().border_color(self.theme.border);
        }

        cell_element.into_any_element()
    }

    fn render_code_block(&self, language: Option<&str>, code: &str) -> AnyElement {
        let mut content = div().w_full().flex().flex_col().gap_1();

        if let Some(language) = language.filter(|lang| !lang.is_empty()) {
            content = content
                .child(self.plain_styled_text(language, self.theme.mono_caption_text_style()));
        }

        for line in code.lines() {
            let line = if line.is_empty() { " " } else { line };
            content = content.child(
                self.plain_styled_text(&line.replace('\t', "    "), self.theme.mono_text_style()),
            );
        }

        if code.is_empty() {
            content = content.child(self.plain_styled_text(" ", self.theme.mono_text_style()));
        }

        div()
            .w_full()
            .bg(self.theme.code_background)
            .p_3()
            .child(content)
            .into_any_element()
    }

    fn render_rich_block(&self, content: &RichText, base_style: TextStyle) -> AnyElement {
        let lines = split_rich_lines(content);
        let children = lines.into_iter().map(|line| {
            div()
                .w_full()
                .child(self.rich_styled_text(&line, base_style.clone()))
                .into_any_element()
        });

        div()
            .w_full()
            .flex()
            .flex_col()
            .children(children)
            .into_any_element()
    }

    fn render_plain_block(&self, text: &str, style: TextStyle) -> AnyElement {
        let lines = split_plain_lines(text);
        let children = lines.into_iter().map(|line| {
            let line = if line.is_empty() {
                " ".to_string()
            } else {
                line
            };
            div()
                .w_full()
                .child(self.plain_styled_text(&line, style.clone()))
                .into_any_element()
        });

        div()
            .w_full()
            .flex()
            .flex_col()
            .children(children)
            .into_any_element()
    }

    fn rich_styled_text(&self, rich_text: &RichText, base_style: TextStyle) -> StyledText {
        if rich_text.segments.is_empty() {
            return StyledText::new(" ".to_string()).with_runs(vec![base_style.to_run(1)]);
        }

        let mut text = String::new();
        let mut runs = Vec::with_capacity(rich_text.segments.len());

        for segment in &rich_text.segments {
            text.push_str(&segment.text);
            let style = self
                .theme
                .apply_inline_style(base_style.clone(), &segment.style);
            runs.push(style.to_run(segment.text.len()));
        }

        StyledText::new(text).with_runs(runs)
    }

    fn plain_styled_text(&self, text: &str, style: TextStyle) -> StyledText {
        let text = if text.is_empty() {
            " ".to_string()
        } else {
            text.to_string()
        };
        let len = text.len();
        StyledText::new(text).with_runs(vec![style.to_run(len)])
    }
}

impl Drop for MarkdownWindow {
    fn drop(&mut self) {
        self.clear_page_assets();
    }
}

impl Render for MarkdownWindow {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        match &self.page {
            PageState::Welcome => self.render_welcome_page(),
            PageState::Document(page) => self.render_document_page(page),
            PageState::Error(page) => self.render_error_page(page),
        }
    }
}

fn clone_app_assets(cx: &mut Context<MarkdownWindow>) -> AppAssets {
    cx.update_global(|assets: &mut AppAssets, _| assets.clone())
}

fn load_page(
    requested_path: PathBuf,
    canonical_path: Option<PathBuf>,
    assets: &AppAssets,
) -> PageState {
    match load_document(&requested_path, canonical_path.as_deref(), assets) {
        Ok((document, asset_prefix)) => PageState::Document(DocumentPage {
            requested_path,
            canonical_path,
            asset_prefix,
            document,
        }),
        Err(error) => PageState::Error(ErrorPage {
            requested_path: Some(requested_path),
            message: error.to_string(),
        }),
    }
}

fn load_document(
    path: &Path,
    canonical_path: Option<&Path>,
    assets: &AppAssets,
) -> Result<(MarkdownDocument, String), LoadError> {
    let bytes = fs::read(path)?;
    let source = String::from_utf8(bytes)?;
    let mut document = parse_markdown(&source);
    let asset_prefix = hydrate_mermaid_blocks(&mut document, path, canonical_path, assets);
    Ok((document, asset_prefix))
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| path_to_lossy_string(path))
}

fn path_to_lossy_string(path: &Path) -> String {
    path.to_string_lossy().replace("\\\\?\\", "")
}

fn split_plain_lines(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = text.split('\n').map(ToOwned::to_owned).collect();
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn split_rich_lines(rich_text: &RichText) -> Vec<RichText> {
    let mut lines = vec![RichText::default()];

    for segment in &rich_text.segments {
        let mut remaining = segment.text.as_str();
        loop {
            if let Some(newline_index) = remaining.find('\n') {
                let (before_newline, after_newline) = remaining.split_at(newline_index);
                if !before_newline.is_empty() {
                    lines
                        .last_mut()
                        .expect("at least one line exists")
                        .push_str(before_newline, segment.style.clone());
                }
                lines.push(RichText::default());
                remaining = &after_newline[1..];
            } else {
                if !remaining.is_empty() {
                    lines
                        .last_mut()
                        .expect("at least one line exists")
                        .push_str(remaining, segment.style.clone());
                }
                break;
            }
        }
    }

    if lines.is_empty() {
        lines.push(RichText::default());
    }

    lines
}

fn table_column_widths(table: &TableBlock) -> Vec<f32> {
    let column_count = table
        .header
        .as_ref()
        .map(|header| header.len())
        .into_iter()
        .chain(table.rows.iter().map(Vec::len))
        .max()
        .unwrap_or(0);

    let mut widths: Vec<f32> = vec![120.0; column_count];

    let mut absorb_row = |row: &[RichText]| {
        for (index, cell) in row.iter().enumerate() {
            let width = table_width_from_text(&cell.plain_text());
            widths[index] = widths[index].max(width);
        }
    };

    if let Some(header) = &table.header {
        absorb_row(header);
    }
    for row in &table.rows {
        absorb_row(row);
    }

    widths
}

fn table_width_from_text(text: &str) -> f32 {
    let score = text
        .lines()
        .map(display_width)
        .max()
        .unwrap_or(0)
        .clamp(6, 40) as f32;

    24.0 + score * 8.0
}

fn table_text_align(alignment: Alignment) -> TextAlign {
    match alignment {
        Alignment::Center => TextAlign::Center,
        Alignment::Right => TextAlign::Right,
        Alignment::Left | Alignment::None => TextAlign::Left,
    }
}

fn display_width(text: &str) -> usize {
    text.chars()
        .map(|ch| {
            if ch == '\t' {
                4
            } else if ch.is_ascii() {
                1
            } else {
                2
            }
        })
        .sum::<usize>()
        .max(1)
}
