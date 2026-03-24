use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag};

#[derive(Clone, Debug, Default)]
pub struct MarkdownDocument {
    pub blocks: Vec<Block>,
    pub title: Option<String>,
}

#[derive(Clone, Debug)]
pub enum Block {
    Heading {
        level: u8,
        content: RichText,
    },
    Paragraph(RichText),
    CodeBlock {
        language: Option<String>,
        code: String,
    },
    Mermaid(MermaidBlock),
    BlockQuote(Vec<Block>),
    List(ListBlock),
    Rule,
    Table(TableBlock),
}

#[derive(Clone, Debug, Default)]
pub struct ListBlock {
    pub ordered: bool,
    pub start_index: usize,
    pub items: Vec<ListItem>,
}

#[derive(Clone, Debug, Default)]
pub struct ListItem {
    pub blocks: Vec<Block>,
    pub task_state: Option<bool>,
}

#[derive(Clone, Debug, Default)]
pub struct TableBlock {
    pub alignments: Vec<Alignment>,
    pub header: Option<Vec<RichText>>,
    pub rows: Vec<Vec<RichText>>,
}

#[derive(Clone, Debug)]
pub struct MermaidBlock {
    pub code: String,
    pub render: MermaidRender,
}

impl MermaidBlock {
    pub fn new(code: String) -> Self {
        Self {
            code,
            render: MermaidRender::Pending,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub enum MermaidRender {
    #[default]
    Pending,
    Rendered {
        asset_path: String,
    },
    Failed {
        message: String,
    },
}

#[derive(Clone, Debug, Default)]
pub struct RichText {
    pub segments: Vec<TextSegment>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextSegment {
    pub text: String,
    pub style: InlineStyle,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InlineStyle {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
    pub link_target: Option<String>,
}

impl RichText {
    pub fn push_str(&mut self, text: &str, style: InlineStyle) {
        if text.is_empty() {
            return;
        }

        if let Some(last) = self.segments.last_mut() {
            if last.style == style {
                last.text.push_str(text);
                return;
            }
        }

        self.segments.push(TextSegment {
            text: text.to_string(),
            style,
        });
    }

    pub fn plain(text: impl AsRef<str>) -> Self {
        let mut rich = Self::default();
        rich.push_str(text.as_ref(), InlineStyle::default());
        rich
    }

    pub fn is_empty(&self) -> bool {
        self.segments.is_empty() || self.segments.iter().all(|segment| segment.text.is_empty())
    }

    pub fn plain_text(&self) -> String {
        let mut text = String::new();
        for segment in &self.segments {
            text.push_str(&segment.text);
        }
        text
    }
}

pub fn parse_markdown(source: &str) -> MarkdownDocument {
    let mut stack = vec![Container::Root { blocks: Vec::new() }];
    let mut styles = vec![InlineStyle::default()];

    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_FOOTNOTES);

    for event in Parser::new_ext(source, options) {
        match event {
            Event::Start(tag) => {
                if !is_inline_tag(&tag) {
                    close_open_paragraph_if_needed(&mut stack);
                }

                match tag {
                    Tag::Paragraph => {
                        stack.push(Container::Leaf {
                            kind: LeafKind::Paragraph,
                            text: RichText::default(),
                        });
                    }
                    Tag::Heading(level, _, _) => {
                        stack.push(Container::Leaf {
                            kind: LeafKind::Heading(heading_level_to_u8(level)),
                            text: RichText::default(),
                        });
                    }
                    Tag::BlockQuote => stack.push(Container::BlockQuote { blocks: Vec::new() }),
                    Tag::FootnoteDefinition(label) => stack.push(Container::Footnote {
                        label: label.to_string(),
                        blocks: Vec::new(),
                    }),
                    Tag::List(start) => stack.push(Container::List {
                        ordered: start.is_some(),
                        start_index: start.unwrap_or(1) as usize,
                        items: Vec::new(),
                    }),
                    Tag::Item => stack.push(Container::ListItem {
                        blocks: Vec::new(),
                        task_state: None,
                    }),
                    Tag::CodeBlock(kind) => stack.push(Container::CodeBlock {
                        language: language_from_code_block(kind),
                        text: String::new(),
                    }),
                    Tag::Table(alignments) => stack.push(Container::Table {
                        alignments,
                        header: None,
                        rows: Vec::new(),
                        current_row: None,
                        current_row_is_header: false,
                    }),
                    Tag::TableHead => {
                        if let Some(Container::Table {
                            current_row,
                            current_row_is_header,
                            ..
                        }) = stack.last_mut()
                        {
                            *current_row = Some(Vec::new());
                            *current_row_is_header = true;
                        }
                    }
                    Tag::TableRow => {
                        if let Some(Container::Table { current_row, .. }) = stack.last_mut() {
                            *current_row = Some(Vec::new());
                        }
                    }
                    Tag::TableCell => stack.push(Container::Leaf {
                        kind: LeafKind::TableCell,
                        text: RichText::default(),
                    }),
                    Tag::Emphasis => push_style(&mut styles, |style| style.italic = true),
                    Tag::Strong => push_style(&mut styles, |style| style.bold = true),
                    Tag::Strikethrough => push_style(&mut styles, |style| style.strike = true),
                    Tag::Link(_, target, _) | Tag::Image(_, target, _) => {
                        let target = target.to_string();
                        push_style(&mut styles, |style| style.link_target = Some(target));
                    }
                }
            }
            Event::End(tag) => match tag {
                Tag::Paragraph | Tag::Heading(_, _, _) | Tag::TableCell => {
                    close_top_leaf(&mut stack)
                }
                Tag::BlockQuote => {
                    close_open_paragraph_if_needed(&mut stack);
                    if let Some(Container::BlockQuote { blocks }) = stack.pop() {
                        push_block(&mut stack, Block::BlockQuote(blocks));
                    }
                }
                Tag::FootnoteDefinition(_) => {
                    close_open_paragraph_if_needed(&mut stack);
                    if let Some(Container::Footnote { label, mut blocks }) = stack.pop() {
                        let mut marker = RichText::default();
                        marker.push_str(
                            &format!("[^{}]", label),
                            InlineStyle {
                                bold: true,
                                ..InlineStyle::default()
                            },
                        );
                        blocks.insert(0, Block::Paragraph(marker));
                        push_block(&mut stack, Block::BlockQuote(blocks));
                    }
                }
                Tag::List(_) => {
                    close_open_paragraph_if_needed(&mut stack);
                    if let Some(Container::List {
                        ordered,
                        start_index,
                        items,
                    }) = stack.pop()
                    {
                        push_block(
                            &mut stack,
                            Block::List(ListBlock {
                                ordered,
                                start_index,
                                items,
                            }),
                        );
                    }
                }
                Tag::Item => {
                    close_open_paragraph_if_needed(&mut stack);
                    if let Some(Container::ListItem { blocks, task_state }) = stack.pop() {
                        push_list_item(&mut stack, ListItem { blocks, task_state });
                    }
                }
                Tag::CodeBlock(_) => {
                    if let Some(Container::CodeBlock { language, text }) = stack.pop() {
                        let block = if language
                            .as_deref()
                            .is_some_and(|language| language.eq_ignore_ascii_case("mermaid"))
                        {
                            Block::Mermaid(MermaidBlock::new(text))
                        } else {
                            Block::CodeBlock {
                                language,
                                code: text,
                            }
                        };

                        push_block(&mut stack, block);
                    }
                }
                Tag::Table(_) => {
                    if let Some(Container::Table {
                        alignments,
                        header,
                        rows,
                        ..
                    }) = stack.pop()
                    {
                        push_block(
                            &mut stack,
                            Block::Table(TableBlock {
                                alignments,
                                header,
                                rows,
                            }),
                        );
                    }
                }
                Tag::TableHead => {
                    finish_current_table_row(&mut stack);
                    if let Some(Container::Table {
                        current_row_is_header,
                        ..
                    }) = stack.last_mut()
                    {
                        *current_row_is_header = false;
                    }
                }
                Tag::TableRow => {
                    finish_current_table_row(&mut stack);
                }
                Tag::Emphasis
                | Tag::Strong
                | Tag::Strikethrough
                | Tag::Link(_, _, _)
                | Tag::Image(_, _, _) => {
                    pop_style(&mut styles);
                }
            },
            Event::Text(text) => append_text(&mut stack, &text, current_style(&styles)),
            Event::Code(code) => {
                let mut style = current_style(&styles);
                style.code = true;
                append_text(&mut stack, &code, style);
            }
            Event::SoftBreak | Event::HardBreak => {
                append_text(&mut stack, "\n", current_style(&styles))
            }
            Event::Html(html) => append_text(&mut stack, &html, current_style(&styles)),
            Event::FootnoteReference(reference) => append_text(
                &mut stack,
                &format!("[^{}]", reference),
                current_style(&styles),
            ),
            Event::Rule => {
                close_open_paragraph_if_needed(&mut stack);
                push_block(&mut stack, Block::Rule);
            }
            Event::TaskListMarker(checked) => {
                if let Some(Container::ListItem { task_state, .. }) = stack.last_mut() {
                    *task_state = Some(checked);
                }
            }
        }
    }

    while matches!(stack.last(), Some(Container::Leaf { .. })) {
        close_top_leaf(&mut stack);
    }

    let blocks = match stack.pop() {
        Some(Container::Root { blocks }) => blocks,
        _ => Vec::new(),
    };

    let title = infer_title(&blocks);
    MarkdownDocument { blocks, title }
}

#[derive(Clone, Debug)]
enum Container {
    Root {
        blocks: Vec<Block>,
    },
    BlockQuote {
        blocks: Vec<Block>,
    },
    Footnote {
        label: String,
        blocks: Vec<Block>,
    },
    List {
        ordered: bool,
        start_index: usize,
        items: Vec<ListItem>,
    },
    ListItem {
        blocks: Vec<Block>,
        task_state: Option<bool>,
    },
    Leaf {
        kind: LeafKind,
        text: RichText,
    },
    CodeBlock {
        language: Option<String>,
        text: String,
    },
    Table {
        alignments: Vec<Alignment>,
        header: Option<Vec<RichText>>,
        rows: Vec<Vec<RichText>>,
        current_row: Option<Vec<RichText>>,
        current_row_is_header: bool,
    },
}

#[derive(Clone, Copy, Debug)]
enum LeafKind {
    Paragraph,
    Heading(u8),
    TableCell,
}

fn blocks_mut(container: &mut Container) -> Option<&mut Vec<Block>> {
    match container {
        Container::Root { blocks }
        | Container::BlockQuote { blocks }
        | Container::Footnote { blocks, .. }
        | Container::ListItem { blocks, .. } => Some(blocks),
        _ => None,
    }
}

fn push_block(stack: &mut [Container], block: Block) {
    if let Some(container) = stack.last_mut() {
        if let Some(blocks) = blocks_mut(container) {
            blocks.push(block);
        }
    }
}

fn push_list_item(stack: &mut [Container], item: ListItem) {
    if let Some(Container::List { items, .. }) = stack.last_mut() {
        items.push(item);
    }
}

fn push_table_cell(stack: &mut [Container], cell: RichText) {
    if let Some(Container::Table {
        current_row: Some(current_row),
        ..
    }) = stack.last_mut()
    {
        current_row.push(cell);
    }
}

fn finish_current_table_row(stack: &mut [Container]) {
    if let Some(Container::Table {
        header,
        rows,
        current_row,
        current_row_is_header,
        ..
    }) = stack.last_mut()
    {
        let Some(finished_row) = current_row.take() else {
            return;
        };

        if *current_row_is_header && header.is_none() {
            *header = Some(finished_row);
        } else {
            rows.push(finished_row);
        }
    }
}

fn close_open_paragraph_if_needed(stack: &mut Vec<Container>) {
    let should_close = matches!(
        stack.last(),
        Some(Container::Leaf {
            kind: LeafKind::Paragraph,
            ..
        })
    );

    if should_close {
        close_top_leaf(stack);
    }
}

fn close_top_leaf(stack: &mut Vec<Container>) {
    let Some(container) = stack.pop() else {
        return;
    };

    match container {
        Container::Leaf { kind, text } => match kind {
            LeafKind::Paragraph => push_block(stack, Block::Paragraph(text)),
            LeafKind::Heading(level) => push_block(
                stack,
                Block::Heading {
                    level,
                    content: text,
                },
            ),
            LeafKind::TableCell => push_table_cell(stack, text),
        },
        other => stack.push(other),
    }
}

fn append_text(stack: &mut Vec<Container>, text: &str, style: InlineStyle) {
    if text.is_empty() {
        return;
    }

    match stack.last_mut() {
        Some(Container::CodeBlock { text: code, .. }) => code.push_str(text),
        Some(Container::Leaf { text: rich, .. }) => rich.push_str(text, style),
        Some(
            Container::Root { .. }
            | Container::BlockQuote { .. }
            | Container::Footnote { .. }
            | Container::ListItem { .. },
        ) => {
            stack.push(Container::Leaf {
                kind: LeafKind::Paragraph,
                text: RichText::default(),
            });
            if let Some(Container::Leaf { text: rich, .. }) = stack.last_mut() {
                rich.push_str(text, style);
            }
        }
        _ => {}
    }
}

fn is_inline_tag(tag: &Tag<'_>) -> bool {
    matches!(
        tag,
        Tag::Emphasis | Tag::Strong | Tag::Strikethrough | Tag::Link(_, _, _) | Tag::Image(_, _, _)
    )
}

fn push_style(styles: &mut Vec<InlineStyle>, update: impl FnOnce(&mut InlineStyle)) {
    let mut next = styles.last().cloned().unwrap_or_default();
    update(&mut next);
    styles.push(next);
}

fn pop_style(styles: &mut Vec<InlineStyle>) {
    if styles.len() > 1 {
        styles.pop();
    }
}

fn current_style(styles: &[InlineStyle]) -> InlineStyle {
    styles.last().cloned().unwrap_or_default()
}

fn language_from_code_block(kind: CodeBlockKind<'_>) -> Option<String> {
    match kind {
        CodeBlockKind::Indented => None,
        CodeBlockKind::Fenced(info) => info
            .split_whitespace()
            .next()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned),
    }
}

fn heading_level_to_u8(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn infer_title(blocks: &[Block]) -> Option<String> {
    let h1 = blocks.iter().find_map(|block| match block {
        Block::Heading { level: 1, content } => normalize_title(content),
        _ => None,
    });

    h1.or_else(|| {
        blocks.iter().find_map(|block| match block {
            Block::Heading { content, .. } => normalize_title(content),
            _ => None,
        })
    })
}

fn normalize_title(content: &RichText) -> Option<String> {
    let title = content.plain_text().trim().to_string();
    (!title.is_empty()).then_some(title)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_heading_and_paragraph() {
        let document = parse_markdown("# Title\n\nhello **world**");
        assert_eq!(document.title.as_deref(), Some("Title"));
        assert!(matches!(
            document.blocks[0],
            Block::Heading { level: 1, .. }
        ));
        assert!(matches!(document.blocks[1], Block::Paragraph(_)));
    }

    #[test]
    fn parses_tight_lists() {
        let document = parse_markdown("- one\n- two\n- three");
        match &document.blocks[0] {
            Block::List(list) => {
                assert_eq!(list.items.len(), 3);
                assert!(matches!(list.items[0].blocks[0], Block::Paragraph(_)));
            }
            other => panic!("expected list, got {other:?}"),
        }
    }

    #[test]
    fn parses_task_lists() {
        let document = parse_markdown("- [x] done\n- [ ] todo");
        match &document.blocks[0] {
            Block::List(list) => {
                assert_eq!(list.items[0].task_state, Some(true));
                assert_eq!(list.items[1].task_state, Some(false));
            }
            other => panic!("expected list, got {other:?}"),
        }
    }

    #[test]
    fn parses_tables() {
        let document = parse_markdown("| a | b |\n| - | -: |\n| 1 | 2 |");
        match &document.blocks[0] {
            Block::Table(table) => {
                assert_eq!(table.header.as_ref().map(Vec::len), Some(2));
                assert_eq!(table.rows.len(), 1);
                assert_eq!(table.rows[0].len(), 2);
            }
            other => panic!("expected table, got {other:?}"),
        }
    }

    #[test]
    fn parses_mermaid_fences_as_mermaid_blocks() {
        let document = parse_markdown("```mermaid\nflowchart LR\nA-->B\n```");
        match &document.blocks[0] {
            Block::Mermaid(block) => {
                assert!(block.code.contains("flowchart LR"));
                assert!(matches!(block.render, MermaidRender::Pending));
            }
            other => panic!("expected mermaid block, got {other:?}"),
        }
    }
}
