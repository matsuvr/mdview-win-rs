use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

#[derive(Clone, Debug, Default)]
pub struct MarkdownDocument {
    pub blocks: Vec<Block>,
    pub title: Option<String>,
}

// ---------------------------------------------------------------------------
// Math types
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct MathBlock {
    pub latex: String,
    pub display: bool,
    pub render: MathRender,
}

#[derive(Clone, Debug)]
pub enum MathRender {
    Pending,
    Rendered { asset_path: String },
    Failed { message: String },
}

impl MathBlock {
    pub fn new(latex: impl Into<String>, display: bool) -> Self {
        Self {
            latex: latex.into(),
            display,
            render: MathRender::Pending,
        }
    }
}

// ---------------------------------------------------------------------------
// Mermaid types
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct MermaidBlock {
    pub code: String,
    pub render: MermaidRender,
}

#[derive(Clone, Debug)]
pub enum MermaidRender {
    Pending,
    Rendered { asset_path: String },
    Failed { message: String },
}

impl MermaidBlock {
    pub fn new(code: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            render: MermaidRender::Pending,
        }
    }
}

// ---------------------------------------------------------------------------
// Block / Segment types
// ---------------------------------------------------------------------------

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
    BlockQuote(Vec<Block>),
    List(ListBlock),
    Rule,
    Table(TableBlock),
    Math(MathBlock),
    Mermaid(MermaidBlock),
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
pub enum Segment {
    Text(TextSegment),
    InlineMath(MathBlock),
}

#[derive(Clone, Debug, Default)]
pub struct RichText {
    pub segments: Vec<Segment>,
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

        if let Some(Segment::Text(last)) = self.segments.last_mut() {
            if last.style == style {
                last.text.push_str(text);
                return;
            }
        }

        self.segments.push(Segment::Text(TextSegment {
            text: text.to_string(),
            style,
        }));
    }

    pub fn plain(text: impl AsRef<str>) -> Self {
        let mut rich = Self::default();
        rich.push_str(text.as_ref(), InlineStyle::default());
        rich
    }

    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
            || self.segments.iter().all(|segment| match segment {
                Segment::Text(t) => t.text.is_empty(),
                Segment::InlineMath(_) => false,
            })
    }

    pub fn plain_text(&self) -> String {
        let mut text = String::new();
        for segment in &self.segments {
            match segment {
                Segment::Text(t) => text.push_str(&t.text),
                Segment::InlineMath(m) => {
                    text.push('$');
                    text.push_str(&m.latex);
                    text.push('$');
                }
            }
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
    options.insert(Options::ENABLE_MATH);

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
                    Tag::Heading { level, .. } => {
                        stack.push(Container::Leaf {
                            kind: LeafKind::Heading(heading_level_to_u8(level)),
                            text: RichText::default(),
                        });
                    }
                    Tag::BlockQuote(_) => {
                        stack.push(Container::BlockQuote { blocks: Vec::new() })
                    }
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
                    Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. } => {
                        let target = dest_url.to_string();
                        push_style(&mut styles, |style| style.link_target = Some(target));
                    }
                    _ => {}
                }
            }
            Event::End(tag) => match tag {
                TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::TableCell => {
                    close_top_leaf(&mut stack)
                }
                TagEnd::BlockQuote(_) => {
                    close_open_paragraph_if_needed(&mut stack);
                    if let Some(Container::BlockQuote { blocks }) = stack.pop() {
                        push_block(&mut stack, Block::BlockQuote(blocks));
                    }
                }
                TagEnd::FootnoteDefinition => {
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
                TagEnd::List(_) => {
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
                TagEnd::Item => {
                    close_open_paragraph_if_needed(&mut stack);
                    if let Some(Container::ListItem { blocks, task_state }) = stack.pop() {
                        push_list_item(&mut stack, ListItem { blocks, task_state });
                    }
                }
                TagEnd::CodeBlock => {
                    if let Some(Container::CodeBlock { language, text }) = stack.pop() {
                        // Mermaid fenced blocks become Block::Mermaid instead of Block::CodeBlock
                        if language.as_deref() == Some("mermaid") {
                            push_block(&mut stack, Block::Mermaid(MermaidBlock::new(text)));
                        } else {
                            push_block(
                                &mut stack,
                                Block::CodeBlock {
                                    language,
                                    code: text,
                                },
                            );
                        }
                    }
                }
                TagEnd::Table => {
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
                TagEnd::TableHead => {
                    finish_current_table_row(&mut stack);
                    if let Some(Container::Table {
                        current_row_is_header,
                        ..
                    }) = stack.last_mut()
                    {
                        *current_row_is_header = false;
                    }
                }
                TagEnd::TableRow => {
                    finish_current_table_row(&mut stack);
                }
                TagEnd::Emphasis
                | TagEnd::Strong
                | TagEnd::Strikethrough
                | TagEnd::Link
                | TagEnd::Image => {
                    pop_style(&mut styles);
                }
                _ => {}
            },
            Event::Text(text) => append_text(&mut stack, &text, current_style(&styles)),
            Event::Code(code) => {
                let mut style = current_style(&styles);
                style.code = true;
                append_text(&mut stack, &code, style);
            }
            Event::InlineMath(latex) => {
                append_inline_math(&mut stack, latex.as_ref());
            }
            Event::DisplayMath(latex) => {
                close_open_paragraph_if_needed(&mut stack);
                push_block(&mut stack, Block::Math(MathBlock::new(latex.as_ref(), true)));
            }
            Event::SoftBreak | Event::HardBreak => {
                append_text(&mut stack, "\n", current_style(&styles))
            }
            Event::Html(html) | Event::InlineHtml(html) => {
                append_text(&mut stack, &html, current_style(&styles))
            }
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

fn append_inline_math(stack: &mut Vec<Container>, latex: &str) {
    let math = MathBlock::new(latex, false);

    match stack.last_mut() {
        Some(Container::Leaf { text: rich, .. }) => {
            rich.segments.push(Segment::InlineMath(math));
        }
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
                rich.segments.push(Segment::InlineMath(math));
            }
        }
        _ => {}
    }
}

fn is_inline_tag(tag: &Tag<'_>) -> bool {
    matches!(
        tag,
        Tag::Emphasis | Tag::Strong | Tag::Strikethrough | Tag::Link { .. } | Tag::Image { .. }
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

    // -----------------------------------------------------------------------
    // Basic structural parsing (pre-existing)
    // -----------------------------------------------------------------------

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

    // -----------------------------------------------------------------------
    // Heading levels 1-6
    // -----------------------------------------------------------------------

    #[test]
    fn parses_all_heading_levels() {
        for level in 1u8..=6 {
            let src = format!("{} Heading {level}", "#".repeat(level as usize));
            let doc = parse_markdown(&src);
            assert!(
                matches!(&doc.blocks[0], Block::Heading { level: l, .. } if *l == level),
                "expected h{level}, got {:?}", doc.blocks[0]
            );
        }
    }

    // -----------------------------------------------------------------------
    // Code blocks
    // -----------------------------------------------------------------------

    #[test]
    fn parses_code_block_with_language() {
        let doc = parse_markdown("```rust\nfn main() {}\n```");
        match &doc.blocks[0] {
            Block::CodeBlock { language, code } => {
                assert_eq!(language.as_deref(), Some("rust"));
                assert!(code.contains("fn main()"));
            }
            other => panic!("expected code block, got {other:?}"),
        }
    }

    #[test]
    fn parses_code_block_python() {
        let doc = parse_markdown("```python\ndef greet(name):\n    return f'Hello, {name}!'\n```");
        match &doc.blocks[0] {
            Block::CodeBlock { language, .. } => {
                assert_eq!(language.as_deref(), Some("python"));
            }
            other => panic!("expected code block, got {other:?}"),
        }
    }

    #[test]
    fn parses_code_block_javascript() {
        let doc = parse_markdown("```javascript\nconst sum = (a, b) => a + b;\n```");
        match &doc.blocks[0] {
            Block::CodeBlock { language, .. } => {
                assert_eq!(language.as_deref(), Some("javascript"));
            }
            other => panic!("expected code block, got {other:?}"),
        }
    }

    #[test]
    fn parses_plain_code_block_no_language() {
        let doc = parse_markdown("```\nsome code\n```");
        match &doc.blocks[0] {
            Block::CodeBlock { language, code } => {
                assert!(language.is_none());
                assert!(code.contains("some code"));
            }
            other => panic!("expected code block, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // Inline styles
    // -----------------------------------------------------------------------

    #[test]
    fn parses_inline_bold() {
        let doc = parse_markdown("**bold text**");
        match &doc.blocks[0] {
            Block::Paragraph(rich) => {
                let bold_seg = rich.segments.iter().find(|s| match s {
                    Segment::Text(t) => t.style.bold,
                    _ => false,
                });
                assert!(bold_seg.is_some(), "expected bold segment");
            }
            other => panic!("expected paragraph, got {other:?}"),
        }
    }

    #[test]
    fn parses_inline_italic() {
        let doc = parse_markdown("*italic text*");
        match &doc.blocks[0] {
            Block::Paragraph(rich) => {
                let italic_seg = rich.segments.iter().find(|s| match s {
                    Segment::Text(t) => t.style.italic,
                    _ => false,
                });
                assert!(italic_seg.is_some(), "expected italic segment");
            }
            other => panic!("expected paragraph, got {other:?}"),
        }
    }

    #[test]
    fn parses_inline_code() {
        let doc = parse_markdown("Use `code` here");
        match &doc.blocks[0] {
            Block::Paragraph(rich) => {
                let code_seg = rich.segments.iter().find(|s| match s {
                    Segment::Text(t) => t.style.code,
                    _ => false,
                });
                assert!(code_seg.is_some(), "expected code segment");
            }
            other => panic!("expected paragraph, got {other:?}"),
        }
    }

    #[test]
    fn parses_strikethrough() {
        let doc = parse_markdown("~~strikethrough~~");
        match &doc.blocks[0] {
            Block::Paragraph(rich) => {
                let strike_seg = rich.segments.iter().find(|s| match s {
                    Segment::Text(t) => t.style.strike,
                    _ => false,
                });
                assert!(strike_seg.is_some(), "expected strikethrough segment");
            }
            other => panic!("expected paragraph, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // Blockquote and Rule
    // -----------------------------------------------------------------------

    #[test]
    fn parses_blockquote() {
        let doc = parse_markdown("> quoted text");
        assert!(matches!(doc.blocks[0], Block::BlockQuote(_)));
    }

    #[test]
    fn parses_horizontal_rule() {
        let doc = parse_markdown("---");
        assert!(matches!(doc.blocks[0], Block::Rule));
    }

    // -----------------------------------------------------------------------
    // Math: Block (display) math  $$...$$
    // -----------------------------------------------------------------------

    #[test]
    fn parses_display_math_block() {
        let doc = parse_markdown("$$e^{i\\pi} + 1 = 0$$");
        let math_block = doc.blocks.iter().find(|b| matches!(b, Block::Math(_)));
        assert!(math_block.is_some(), "expected Block::Math, got: {:?}", doc.blocks);
    }

    #[test]
    fn parses_display_math_is_display_mode() {
        let doc = parse_markdown("$$x^2$$");
        match doc.blocks.iter().find(|b| matches!(b, Block::Math(_))) {
            Some(Block::Math(m)) => assert!(m.display, "expected display=true"),
            _ => panic!("no Block::Math found"),
        }
    }

    #[test]
    fn parses_display_math_euler_identity() {
        let doc = parse_markdown("$$e^{i\\pi} + 1 = 0$$");
        match doc.blocks.iter().find(|b| matches!(b, Block::Math(_))) {
            Some(Block::Math(m)) => {
                assert!(m.display, "expected display mode");
                assert!(m.latex.contains("pi") || m.latex.contains("\\pi"),
                    "expected pi in formula, got: {}", m.latex);
            }
            _ => panic!("no Block::Math found"),
        }
    }

    #[test]
    fn parses_display_math_hamiltonian() {
        let doc = parse_markdown("$$\\hat{H}\\psi = E\\psi$$");
        match doc.blocks.iter().find(|b| matches!(b, Block::Math(_))) {
            Some(Block::Math(m)) => {
                assert!(m.display);
                assert!(m.latex.contains("H") && m.latex.contains("psi") || m.latex.contains("\\psi"),
                    "hamiltonian formula content unexpected: {}", m.latex);
            }
            _ => panic!("no Block::Math found"),
        }
    }

    #[test]
    fn parses_display_math_fourier_transform() {
        // $$\hat{f}(\xi) = \int_{-\infty}^{\infty} f(x) e^{-2\pi i x \xi} dx$$
        let doc = parse_markdown("$$\\hat{f}(\\xi) = \\int_{-\\infty}^{\\infty} f(x) e^{-2\\pi i x \\xi} dx$$");
        let found = doc.blocks.iter().any(|b| matches!(b, Block::Math(m) if m.display));
        assert!(found, "expected display math block for Fourier transform");
    }

    #[test]
    fn parses_multiple_display_math_blocks() {
        // Maxwell equations: 4 separate $$...$$ blocks
        let src = "$$\\nabla \\cdot \\mathbf{E} = \\frac{\\rho}{\\varepsilon_0}$$\n\n\
                   $$\\nabla \\cdot \\mathbf{B} = 0$$\n\n\
                   $$\\nabla \\times \\mathbf{E} = -\\frac{\\partial \\mathbf{B}}{\\partial t}$$\n\n\
                   $$\\nabla \\times \\mathbf{B} = \\mu_0 \\mathbf{J} + \\mu_0 \\varepsilon_0 \\frac{\\partial \\mathbf{E}}{\\partial t}$$";
        let doc = parse_markdown(src);
        let math_count = doc.blocks.iter().filter(|b| matches!(b, Block::Math(_))).count();
        assert_eq!(math_count, 4, "expected 4 math blocks for Maxwell equations, got {math_count}");
    }

    #[test]
    fn parses_display_math_gaussian_integral() {
        let doc = parse_markdown("$$\\int_{-\\infty}^{\\infty} e^{-x^2} dx = \\sqrt{\\pi}$$");
        let found = doc.blocks.iter().any(|b| matches!(b, Block::Math(m) if m.display));
        assert!(found, "expected display math block for Gaussian integral");
    }

    #[test]
    fn parses_display_math_sum_notation() {
        let doc = parse_markdown("$$\\sum_{i=1}^{n} x_i$$");
        let found = doc.blocks.iter().any(|b| matches!(b, Block::Math(m) if m.display));
        assert!(found, "expected display math block for sum notation");
    }

    // -----------------------------------------------------------------------
    // Math: Inline math  $...$
    // -----------------------------------------------------------------------

    #[test]
    fn parses_inline_math_segment() {
        let doc = parse_markdown("The formula $E = mc^2$ is famous.");
        match &doc.blocks[0] {
            Block::Paragraph(rich) => {
                let has_inline = rich.segments.iter().any(|s| matches!(s, Segment::InlineMath(_)));
                assert!(has_inline, "expected inline math segment, got: {:?}", rich.segments);
            }
            other => panic!("expected paragraph, got {other:?}"),
        }
    }

    #[test]
    fn parses_inline_math_is_not_display_mode() {
        let doc = parse_markdown("See $x^2 + y^2 = r^2$ here.");
        match &doc.blocks[0] {
            Block::Paragraph(rich) => {
                match rich.segments.iter().find(|s| matches!(s, Segment::InlineMath(_))) {
                    Some(Segment::InlineMath(m)) => assert!(!m.display, "inline math should not be display mode"),
                    _ => panic!("no inline math segment found"),
                }
            }
            other => panic!("expected paragraph, got {other:?}"),
        }
    }

    #[test]
    fn parses_inline_math_quadratic_formula() {
        // From README: $x = \frac{-b \pm \sqrt{b^2 - 4ac}}{2a}$
        let doc = parse_markdown("二次方程式の解の公式は $x = \\frac{-b \\pm \\sqrt{b^2 - 4ac}}{2a}$ です。");
        match &doc.blocks[0] {
            Block::Paragraph(rich) => {
                let has_inline = rich.segments.iter().any(|s| matches!(s, Segment::InlineMath(_)));
                assert!(has_inline, "expected inline math for quadratic formula");
            }
            other => panic!("expected paragraph, got {other:?}"),
        }
    }

    #[test]
    fn parses_inline_math_pi_and_e() {
        // $\pi \approx 3.14159$ and $e \approx 2.71828$
        let doc = parse_markdown("円周率は $\\pi \\approx 3.14159$、ネイピア数は $e \\approx 2.71828$ です。");
        match &doc.blocks[0] {
            Block::Paragraph(rich) => {
                let count = rich.segments.iter().filter(|s| matches!(s, Segment::InlineMath(_))).count();
                assert_eq!(count, 2, "expected 2 inline math segments, got {count}");
            }
            other => panic!("expected paragraph, got {other:?}"),
        }
    }

    #[test]
    fn parses_inline_math_planck_constant() {
        // $\hbar$ - reduced Planck constant
        let doc = parse_markdown("The reduced Planck constant is $\\hbar$.");
        match &doc.blocks[0] {
            Block::Paragraph(rich) => {
                match rich.segments.iter().find(|s| matches!(s, Segment::InlineMath(_))) {
                    Some(Segment::InlineMath(m)) => {
                        assert!(m.latex.contains("hbar") || m.latex.contains("\\hbar"),
                            "expected hbar in formula, got: {}", m.latex);
                    }
                    _ => panic!("no inline math segment found"),
                }
            }
            other => panic!("expected paragraph, got {other:?}"),
        }
    }

    #[test]
    fn parses_inline_math_tensor_notation() {
        // $T^{\mu}_{\nu}$ - tensor
        let doc = parse_markdown("The tensor $T^{\\mu}_{\\nu}$ represents the stress-energy tensor.");
        match &doc.blocks[0] {
            Block::Paragraph(rich) => {
                let has_inline = rich.segments.iter().any(|s| matches!(s, Segment::InlineMath(_)));
                assert!(has_inline, "expected inline math for tensor notation");
            }
            other => panic!("expected paragraph, got {other:?}"),
        }
    }

    #[test]
    fn parses_inline_math_hat_notation() {
        // $\hat{f}$ - operator hat
        let doc = parse_markdown("The Fourier transform $\\hat{f}(\\xi)$ of a function.");
        match &doc.blocks[0] {
            Block::Paragraph(rich) => {
                match rich.segments.iter().find(|s| matches!(s, Segment::InlineMath(_))) {
                    Some(Segment::InlineMath(m)) => {
                        assert!(m.latex.contains("hat") || m.latex.contains("\\hat"),
                            "expected hat in formula, got: {}", m.latex);
                    }
                    _ => panic!("no inline math segment found"),
                }
            }
            other => panic!("expected paragraph, got {other:?}"),
        }
    }

    #[test]
    fn parses_paragraph_with_mixed_text_and_math() {
        let doc = parse_markdown("The mass-energy formula $E = mc^2$ was discovered by Einstein.");
        match &doc.blocks[0] {
            Block::Paragraph(rich) => {
                // should have at least one text segment and one inline math segment
                let has_text = rich.segments.iter().any(|s| matches!(s, Segment::Text(_)));
                let has_math = rich.segments.iter().any(|s| matches!(s, Segment::InlineMath(_)));
                assert!(has_text, "expected text segment");
                assert!(has_math, "expected inline math segment");
            }
            other => panic!("expected paragraph, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // Mermaid: fenced code blocks with language "mermaid"
    // -----------------------------------------------------------------------

    #[test]
    fn parses_mermaid_fenced_block_as_mermaid_variant() {
        let doc = parse_markdown("```mermaid\ngraph TD\n    A[Hello] --> B[World]\n```");
        let mermaid = doc.blocks.iter().find(|b| matches!(b, Block::Mermaid(_)));
        assert!(mermaid.is_some(), "expected Block::Mermaid, got: {:?}", doc.blocks);
    }

    #[test]
    fn parses_mermaid_not_as_code_block() {
        let doc = parse_markdown("```mermaid\ngraph TD\n    A --> B\n```");
        let code_block = doc.blocks.iter().find(|b| {
            matches!(b, Block::CodeBlock { language: Some(l), .. } if l == "mermaid")
        });
        assert!(code_block.is_none(), "mermaid blocks should not be CodeBlock, they should be Mermaid blocks");
    }

    #[test]
    fn parses_mermaid_code_preserved() {
        let code = "graph TD\n    A[Hello] --> B[World]";
        let doc = parse_markdown(&format!("```mermaid\n{code}\n```"));
        match doc.blocks.iter().find(|b| matches!(b, Block::Mermaid(_))) {
            Some(Block::Mermaid(m)) => {
                assert!(m.code.contains("graph TD"), "expected mermaid code, got: {}", m.code);
                assert!(m.code.contains("A[Hello]"), "expected node A");
            }
            _ => panic!("no Mermaid block found"),
        }
    }

    #[test]
    fn parses_mermaid_flowchart_from_readme() {
        let src = "```mermaid\nflowchart TD\n    A[ファイルを開く] --> B{ファイル存在?}\n    B -->|はい| C[Markdown パース]\n    B -->|いいえ| D[エラー表示]\n    C --> E[Mermaid/数式 変換]\n    E --> F[GPUI で描画]\n```";
        let doc = parse_markdown(src);
        let found = doc.blocks.iter().any(|b| matches!(b, Block::Mermaid(m) if m.code.contains("flowchart")));
        assert!(found, "expected flowchart mermaid block");
    }

    #[test]
    fn parses_mermaid_sequence_diagram() {
        let src = "```mermaid\nsequenceDiagram\n    Alice->>Bob: Hello\n    Bob-->>Alice: Hi!\n```";
        let doc = parse_markdown(src);
        let found = doc.blocks.iter().any(|b| matches!(b, Block::Mermaid(m) if m.code.contains("sequenceDiagram")));
        assert!(found, "expected sequenceDiagram mermaid block");
    }

    #[test]
    fn parses_mermaid_class_diagram() {
        let src = "```mermaid\nclassDiagram\n    class Document {\n        +parse()\n    }\n```";
        let doc = parse_markdown(src);
        let found = doc.blocks.iter().any(|b| matches!(b, Block::Mermaid(m) if m.code.contains("classDiagram")));
        assert!(found, "expected classDiagram mermaid block");
    }

    #[test]
    fn parses_mermaid_initial_render_state_is_pending() {
        let doc = parse_markdown("```mermaid\ngraph TD\n    A --> B\n```");
        match doc.blocks.iter().find(|b| matches!(b, Block::Mermaid(_))) {
            Some(Block::Mermaid(m)) => {
                assert!(matches!(m.render, MermaidRender::Pending), "initial render state should be Pending");
            }
            _ => panic!("no Mermaid block found"),
        }
    }

    #[test]
    fn parses_display_math_initial_render_state_is_pending() {
        let doc = parse_markdown("$$x^2$$");
        match doc.blocks.iter().find(|b| matches!(b, Block::Math(_))) {
            Some(Block::Math(m)) => {
                assert!(matches!(m.render, MathRender::Pending), "initial render state should be Pending");
            }
            _ => panic!("no Math block found"),
        }
    }

    // -----------------------------------------------------------------------
    // Document-level parsing for the full test_math.md content
    // -----------------------------------------------------------------------

    #[test]
    fn parse_test_math_md_block_formulas() {
        let src = "$$E = mc^2$$\n\nThis is an inline formula: $E = mc^2$ in text.\n\n$$\\frac{a}{b} + \\sqrt{x}$$\n\nAnother display math:\n\n$$\\sum_{i=1}^{n} x_i$$";
        let doc = parse_markdown(src);
        let math_count = doc.blocks.iter().filter(|b| matches!(b, Block::Math(_))).count();
        assert_eq!(math_count, 3, "expected 3 display math blocks, got {math_count}");
    }

    #[test]
    fn parse_test_math_md_inline_formula() {
        let src = "This is an inline formula: $E = mc^2$ in text.";
        let doc = parse_markdown(src);
        match &doc.blocks[0] {
            Block::Paragraph(rich) => {
                let has_inline = rich.segments.iter().any(|s| matches!(s, Segment::InlineMath(_)));
                assert!(has_inline, "expected inline math in paragraph");
            }
            other => panic!("expected paragraph, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // Full README.md-style parsing
    // -----------------------------------------------------------------------

    #[test]
    fn parse_readme_style_document_has_title() {
        let src = "# mdview\n\nUltra-simple markdown viewer.";
        let doc = parse_markdown(src);
        assert_eq!(doc.title.as_deref(), Some("mdview"));
    }

    #[test]
    fn parse_readme_style_document_table() {
        let src = "| ファイル | 役割 |\n|---|---|\n| `src/main.rs` | エントリポイント |";
        let doc = parse_markdown(src);
        assert!(matches!(doc.blocks[0], Block::Table(_)), "expected table block");
    }

    #[test]
    fn parse_readme_with_math_and_mermaid() {
        let src = "# Test\n\n$$e^{i\\pi} + 1 = 0$$\n\n```mermaid\ngraph TD\n    A --> B\n```";
        let doc = parse_markdown(src);
        assert!(doc.blocks.iter().any(|b| matches!(b, Block::Math(_))), "expected math block");
        assert!(doc.blocks.iter().any(|b| matches!(b, Block::Mermaid(_))), "expected mermaid block");
    }

    #[test]
    fn parse_readme_mixed_content_block_order() {
        // heading → paragraph → math → mermaid
        let src = "## Section\n\nSome text with $x$ inline.\n\n$$y = mx + b$$\n\n```mermaid\ngraph TD\n    A --> B\n```";
        let doc = parse_markdown(src);
        let has_heading = doc.blocks.iter().any(|b| matches!(b, Block::Heading { level: 2, .. }));
        let has_paragraph = doc.blocks.iter().any(|b| matches!(b, Block::Paragraph(_)));
        let has_math = doc.blocks.iter().any(|b| matches!(b, Block::Math(_)));
        let has_mermaid = doc.blocks.iter().any(|b| matches!(b, Block::Mermaid(_)));
        assert!(has_heading, "expected h2");
        assert!(has_paragraph, "expected paragraph");
        assert!(has_math, "expected block math");
        assert!(has_mermaid, "expected mermaid");
    }
}
