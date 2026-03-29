use std::{
    hash::{Hash, Hasher},
    path::Path,
    sync::{Arc, LazyLock, OnceLock},
};

use fontdb::Database;
use resvg::usvg::{Options, Tree};
use resvg::tiny_skia::{Color, Pixmap};
use typst::{
    compile,
    diag::{FileError, FileResult},
    foundations::{Bytes, Datetime},
    layout::PagedDocument,
    text::{Font, FontBook},
    Library, LibraryExt, World,
};
use typst::syntax::{FileId, Source, VirtualPath};
use typst::utils::LazyHash;

use crate::{
    assets::AppAssets,
    markdown::{Block, MathBlock, MathRender, MarkdownDocument, RichText, Segment},
};

/// Global font database loaded once at first use.
/// Loads fonts from Windows system directories for multilingual support.
static FONTDB: OnceLock<Arc<Database>> = OnceLock::new();

/// Get or initialize the font database.
fn get_fontdb() -> Arc<Database> {
    FONTDB.get_or_init(|| {
        let mut db = Database::new();

        // Load system fonts from Windows Fonts directory
        let system_fonts = Path::new("C:\\Windows\\Fonts");
        if system_fonts.exists() {
            db.load_fonts_dir(system_fonts);
        }

        // Load user-installed fonts (Windows 10 1809+)
        if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
            let user_fonts = Path::new(&local_app_data).join("Microsoft\\Windows\\Fonts");
            if user_fonts.exists() {
                db.load_fonts_dir(&user_fonts);
            }
        }

        Arc::new(db)
    }).clone()
}

// ---------------------------------------------------------------------------
// Public API (mirrors mermaid.rs)
// ---------------------------------------------------------------------------

pub fn hydrate_math_blocks(
    document: &mut MarkdownDocument,
    requested_path: &Path,
    canonical_path: Option<&Path>,
    assets: &AppAssets,
) -> String {
    let prefix = math_asset_prefix(requested_path, canonical_path);
    assets.remove_prefix(&prefix);

    let mut block_index = 0usize;
    hydrate_blocks(&mut document.blocks, &prefix, assets, &mut block_index);
    prefix
}

// ---------------------------------------------------------------------------
// Block tree walk
// ---------------------------------------------------------------------------

fn hydrate_blocks(
    blocks: &mut [Block],
    prefix: &str,
    assets: &AppAssets,
    block_index: &mut usize,
) {
    for block in blocks {
        match block {
            Block::Math(math) => {
                render_and_store(math, prefix, assets, block_index);
            }
            Block::Heading { content, .. } | Block::Paragraph(content) => {
                hydrate_rich_text(content, prefix, assets, block_index);
            }
            Block::BlockQuote(children) => hydrate_blocks(children, prefix, assets, block_index),
            Block::List(list) => {
                for item in &mut list.items {
                    hydrate_blocks(&mut item.blocks, prefix, assets, block_index);
                }
            }
            Block::Table(table) => {
                if let Some(header) = &mut table.header {
                    for cell in header {
                        hydrate_rich_text(cell, prefix, assets, block_index);
                    }
                }
                for row in &mut table.rows {
                    for cell in row {
                        hydrate_rich_text(cell, prefix, assets, block_index);
                    }
                }
            }
            Block::CodeBlock { .. } | Block::Mermaid(_) | Block::Rule => {}
        }
    }
}

fn hydrate_rich_text(
    rich: &mut RichText,
    prefix: &str,
    assets: &AppAssets,
    block_index: &mut usize,
) {
    for segment in &mut rich.segments {
        if let Segment::InlineMath(math) = segment {
            render_and_store(math, prefix, assets, block_index);
        }
    }
}

fn render_and_store(
    math: &mut MathBlock,
    prefix: &str,
    assets: &AppAssets,
    block_index: &mut usize,
) {
    let asset_path = format!(
        "{prefix}math-{index:04}-{hash:016x}.png",
        index = *block_index,
        hash = stable_hash(&math.latex),
    );

    match render_latex_to_png(&math.latex, math.display) {
        Ok(png_data) => {
            eprintln!(
                "[math] Rendered {} bytes for '{}' ({}) -> {}",
                png_data.len(),
                &math.latex,
                if math.display { "display" } else { "inline" },
                &asset_path
            );
            // Store in asset system (like mermaid does with SVG)
            assets.insert_bytes(asset_path.clone(), png_data);
            math.render = MathRender::Rendered { asset_path };
        }
        Err(message) => {
            eprintln!(
                "[math] FAILED to render '{}': {}",
                &math.latex, &message
            );
            math.render = MathRender::Failed { message };
        }
    }

    *block_index += 1;
}

// ---------------------------------------------------------------------------
// Typst rendering
// ---------------------------------------------------------------------------

fn render_latex_to_png(latex: &str, display: bool) -> Result<Vec<u8>, String> {
    let source_text = build_typst_source(latex, display);
    let source = Source::new(*MAIN_ID, source_text);

    let world = MathWorld::new(source);

    let warned = compile::<PagedDocument>(&world);
    let document = warned.output.map_err(|diagnostics| {
        let errors: Vec<String> = diagnostics
            .iter()
            .map(|d| d.message.to_string())
            .collect();
        errors.join("; ")
    })?;

    let page = document
        .pages
        .first()
        .ok_or_else(|| "Typst produced no pages".to_string())?;

    // Get SVG from Typst
    let svg = typst_svg::svg(page);

    // Parse SVG with usvg, using our font database
    let fontdb = get_fontdb();
    let mut options = Options::default();
    options.fontdb = fontdb;

    let tree = Tree::from_str(&svg, &options)
        .map_err(|e| format!("Failed to parse SVG: {}", e))?;

    // Get the bounding box
    let size = tree.size();
    let width = size.width() as u32;
    let height = size.height() as u32;

    // Create a pixmap (render target)
    let mut pixmap = Pixmap::new(width, height)
        .ok_or_else(|| "Failed to create pixmap".to_string())?;
    pixmap.fill(Color::from_rgba8(255, 255, 255, 255));

    // Render the SVG to the pixmap
    resvg::render(&tree, resvg::tiny_skia::Transform::identity(), &mut pixmap.as_mut());

    // Encode to PNG
    let png_data = pixmap.encode_png()
        .map_err(|e| format!("Failed to encode PNG: {}", e))?;

    Ok(png_data)
}

/// Build a minimal Typst source document that renders a math formula.
///
/// Typst math mode understands many LaTeX-like commands natively (Greek
/// letters without backslash, `frac(a, b)`, `sqrt(x)`, etc.).  We perform
/// lightweight preprocessing of common LaTeX patterns so that standard KaTeX
/// inputs "just work" for the most common cases.
fn build_typst_source(latex: &str, display: bool) -> String {
    let converted = preprocess_latex(latex);

    if display {
        format!(
            "#set page(width: auto, height: auto, margin: 0.5em)\n\
             #set text(size: 14pt, fill: rgb(\"#1f2328\"))\n\
             $ {} $",
            converted
        )
    } else {
        format!(
            "#set page(width: auto, height: auto, margin: (x: 0.08em, y: 0.03em))\n\
             #set text(size: 11pt, fill: rgb(\"#1f2328\"))\n\
             ${}$",
            converted
        )
    }
}

/// Lightweight LaTeX → Typst math syntax preprocessing.
///
/// This does NOT aim for 100% LaTeX compatibility.  It converts the most
/// common constructs so that typical KaTeX-style formulas render correctly.
fn preprocess_latex(latex: &str) -> String {
    let mut result = latex.to_string();

    // \hat{x} → hat(x)  — operator hat (e.g., \hat{H} in quantum mechanics)
    result = replace_command_one_arg(&result, "hat", "hat");
    // \widehat{x} → hat(x)  — wide hat variant
    result = replace_command_one_arg(&result, "widehat", "hat");
    // \tilde{x} → tilde(x)
    result = replace_command_one_arg(&result, "tilde", "tilde");
    // \vec{x} → arrow(x)
    result = replace_command_one_arg(&result, "vec", "arrow");
    // \bar{x} → overline(x)
    result = replace_command_one_arg(&result, "bar", "overline");
    // \hbar → ℏ  (reduced Planck constant, use Unicode so Typst renders it)
    result = result.replace("\\hbar", "ℏ");
    // \frac{a}{b} → frac(a, b)
    result = replace_command_two_args(&result, "frac");
    // \sqrt{x} → sqrt(x)   and  \sqrt[n]{x} → root(n, x)
    result = replace_sqrt(&result);
    // \text{...} → upright(...)
    result = replace_command_one_arg(&result, "text", "upright");
    // \mathrm{...} → upright(...)
    result = replace_command_one_arg(&result, "mathrm", "upright");
    // \mathbf{...} → bold(...)
    result = replace_command_one_arg(&result, "mathbf", "bold");
    // \mathcal{...} → cal(...)
    result = replace_command_one_arg(&result, "mathcal", "cal");
    // \left and \right delimiters → just the delimiter
    result = result.replace("\\left", "");
    result = result.replace("\\right", "");
    // \cdot → dot
    result = result.replace("\\cdot", "dot");
    // \cdots → dots.c
    result = result.replace("\\cdots", "dots.c");
    // \ldots → dots
    result = result.replace("\\ldots", "dots");
    // \times → times
    result = result.replace("\\times", "times");
    // \pm → plus.minus
    result = result.replace("\\pm", "plus.minus");
    // \mp → minus.plus
    result = result.replace("\\mp", "minus.plus");
    // \neq → eq.not
    result = result.replace("\\neq", "eq.not");
    // \ne → eq.not
    result = result.replace("\\ne ", "eq.not ");
    // \leq → lt.eq
    result = result.replace("\\leq", "lt.eq");
    // \geq → gt.eq
    result = result.replace("\\geq", "gt.eq");
    // \le → lt.eq
    result = result.replace("\\le ", "lt.eq ");
    // \ge → gt.eq
    result = result.replace("\\ge ", "gt.eq ");
    // \approx → approx
    result = result.replace("\\approx", "approx");
    // \infty → infinity
    result = result.replace("\\infty", "infinity");
    // \partial → diff
    result = result.replace("\\partial", "diff");
    // \nabla → nabla
    result = result.replace("\\nabla", "nabla");
    // \int → integral
    result = result.replace("\\int", "integral");
    // \sum → sum
    result = result.replace("\\sum", "sum");
    // \prod → product
    result = result.replace("\\prod", "product");
    // \lim → lim
    result = result.replace("\\lim", "lim");
    // \to → arrow.r
    result = result.replace("\\to ", "arrow.r ");
    // \rightarrow → arrow.r
    result = result.replace("\\rightarrow", "arrow.r");
    // \leftarrow → arrow.l
    result = result.replace("\\leftarrow", "arrow.l");
    // \Rightarrow → arrow.r.double
    result = result.replace("\\Rightarrow", "arrow.r.double");
    // \in → in
    result = result.replace("\\in ", "in ");
    // \notin → in.not
    result = result.replace("\\notin", "in.not");
    // \subset → subset
    result = result.replace("\\subset", "subset");
    // \supset → supset
    result = result.replace("\\supset", "supset");
    // \cup → union
    result = result.replace("\\cup", "union");
    // \cap → sect
    result = result.replace("\\cap", "sect");
    // \forall → forall
    result = result.replace("\\forall", "forall");
    // \exists → exists
    result = result.replace("\\exists", "exists");

    // \begin{pmatrix}...\end{pmatrix} → mat(delim: "(", ...; ...)
    result = replace_matrix_env(&result, "pmatrix", "\"(\"");
    result = replace_matrix_env(&result, "bmatrix", "\"[\"");
    result = replace_matrix_env(&result, "vmatrix", "\"|\"");
    result = replace_matrix_env(&result, "matrix", "\"(\"");

    // Greek: strip backslash from known Greek letters.
    // Typst uses bare names: alpha, beta, gamma, etc.
    // Handle variant forms first, mapping to Unicode so Typst renders them correctly.
    result = result.replace("\\varepsilon", " ε");
    result = result.replace("\\varphi", " φ");
    result = result.replace("\\vartheta", " ϑ");
    for letter in GREEK_LETTERS {
        let with_backslash = format!("\\{letter}");
        // Add a leading space so that e.g. `i\pi` → `i pi` (not `ipi`).
        // Typst math mode ignores extra whitespace, so this is safe.
        result = result.replace(&with_backslash, &format!(" {letter}"));
    }

    // Strip remaining \operatorname{...} → just contents
    result = replace_command_one_arg(&result, "operatorname", "");

    // Split multi-letter identifiers like "mc" into "m c"
    // This is needed because Typst treats "mc" as a single identifier,
    // but in LaTeX it means implicit multiplication: m * c
    result = split_multi_letter_identifiers(&result);
    result = convert_grouped_scripts(&result);

    // Remaining \command patterns that Typst might understand if the
    // backslash is simply removed (many operator names)
    // We do this as a final fallback for common commands.

    result
}

/// Split 2-letter lowercase identifiers into separate letters when they are
/// not known Typst symbols or Greek letters.
/// This handles cases like "mc" -> "m c" for implicit multiplication in
/// physics formulas (e.g. E = mc²), while preserving all Typst function names.
fn split_multi_letter_identifiers(input: &str) -> String {
    // Two-letter words that must NOT be split (Greek abbreviations, Typst operators)
    static PRESERVE_2: &[&str] = &[
        // Greek 2-letter names
        "mu", "nu", "pi", "xi",
        // Typst math operators / symbol prefixes
        "eq", "lt", "gt", "in", "or", "or", "lr",
        // Common physics 2-letter identifiers that are single symbols
        "ij", "kl", "mn",
    ];

    let mut result = String::with_capacity(input.len() * 2);
    let mut chars = input.char_indices().peekable();
    let mut current_word = String::new();

    while let Some((_i, ch)) = chars.next() {
        if ch.is_ascii_lowercase() {
            current_word.push(ch);
            // Check if next char is also lowercase (continuing the word)
            if chars.peek().map(|(_, c)| c.is_ascii_lowercase()).unwrap_or(false) {
                continue;
            }
            // Word ended — only split if it is exactly 2 letters and not preserved
            if current_word.len() == 2 && !PRESERVE_2.contains(&current_word.as_str()) {
                let mut letters = current_word.chars();
                result.push(letters.next().unwrap());
                result.push(' ');
                result.push(letters.next().unwrap());
            } else {
                result.push_str(&current_word);
            }
            current_word.clear();
        } else {
            if !current_word.is_empty() {
                result.push_str(&current_word);
                current_word.clear();
            }
            result.push(ch);
        }
    }

    if !current_word.is_empty() {
        result.push_str(&current_word);
    }

    result
}

#[derive(Default)]
struct ScriptChain {
    superscript: Option<String>,
    subscript: Option<String>,
    raw: String,
    has_grouped: bool,
}

fn convert_grouped_scripts(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut index = 0usize;

    while index < input.len() {
        let (atom_end, scriptable) = parse_math_atom(input, index);
        if atom_end == index {
            break;
        }

        let atom = &input[index..atom_end];
        if !scriptable {
            result.push_str(atom);
            index = atom_end;
            continue;
        }

        let (scripts, next_index) = parse_script_chain(input, atom_end);
        if scripts.has_grouped {
            result.push_str(&build_attach_call(atom.trim(), &scripts));
        } else {
            result.push_str(atom);
            result.push_str(&scripts.raw);
        }

        index = next_index;
    }

    result
}

fn parse_script_chain(input: &str, mut index: usize) -> (ScriptChain, usize) {
    let mut chain = ScriptChain::default();

    while index < input.len() {
        let op = match input[index..].chars().next() {
            Some('^') => '^',
            Some('_') => '_',
            _ => break,
        };

        let op_start = index;
        index += 1;

        let arg_start = skip_whitespace(input, index);
        let Some((argument, next_index, grouped)) = parse_script_argument(input, arg_start) else {
            index = op_start;
            break;
        };

        chain.raw.push_str(&input[op_start..next_index]);
        chain.has_grouped |= grouped;

        match op {
            '^' if chain.superscript.is_none() => chain.superscript = Some(argument),
            '_' if chain.subscript.is_none() => chain.subscript = Some(argument),
            _ => {}
        }

        index = next_index;
    }

    (chain, index)
}

fn parse_script_argument(input: &str, start: usize) -> Option<(String, usize, bool)> {
    let next = input[start..].chars().next()?;
    if next == '{' {
        let end = consume_balanced_group(input, start, '{', '}')?;
        let content = &input[start + 1..end - 1];
        return Some((convert_grouped_scripts(content.trim()), end, true));
    }

    let (end, _) = parse_math_atom(input, start);
    if end == start {
        return None;
    }

    Some((input[start..end].to_string(), end, false))
}

fn parse_math_atom(input: &str, start: usize) -> (usize, bool) {
    let Some(ch) = input[start..].chars().next() else {
        return (start, false);
    };

    if ch.is_whitespace() {
        let end = input[start..]
            .char_indices()
            .find(|(_, c)| !c.is_whitespace())
            .map(|(offset, _)| start + offset)
            .unwrap_or(input.len());
        return (end, false);
    }

    if ch == '"' {
        return (consume_quoted_string(input, start), true);
    }

    if matches!(ch, '(' | '[') {
        let close = if ch == '(' { ')' } else { ']' };
        let end = consume_balanced_group(input, start, ch, close)
            .unwrap_or(start + ch.len_utf8());
        return (end, true);
    }

    if is_math_identifier_char(ch) {
        let mut end = start + ch.len_utf8();
        while end < input.len() {
            let next = input[end..].chars().next().unwrap();
            if is_math_identifier_char(next) {
                end += next.len_utf8();
            } else {
                break;
            }
        }

        while end < input.len() {
            let next = input[end..].chars().next().unwrap();
            let close = match next {
                '(' => ')',
                '[' => ']',
                _ => break,
            };
            let Some(group_end) = consume_balanced_group(input, end, next, close) else {
                break;
            };
            end = group_end;
        }

        return (end, true);
    }

    (start + ch.len_utf8(), false)
}

fn build_attach_call(base: &str, scripts: &ScriptChain) -> String {
    let mut result = format!("attach({base}");
    if let Some(subscript) = &scripts.subscript {
        result.push_str(", b: ");
        result.push_str(subscript.trim());
    }
    if let Some(superscript) = &scripts.superscript {
        result.push_str(", t: ");
        result.push_str(superscript.trim());
    }
    result.push(')');
    result
}

fn skip_whitespace(input: &str, mut index: usize) -> usize {
    while index < input.len() {
        let ch = input[index..].chars().next().unwrap();
        if !ch.is_whitespace() {
            break;
        }
        index += ch.len_utf8();
    }
    index
}

fn consume_balanced_group(input: &str, start: usize, open: char, close: char) -> Option<usize> {
    let mut depth = 0i32;
    for (offset, ch) in input[start..].char_indices() {
        if ch == open {
            depth += 1;
        } else if ch == close {
            depth -= 1;
            if depth == 0 {
                return Some(start + offset + ch.len_utf8());
            }
        }
    }
    None
}

fn consume_quoted_string(input: &str, start: usize) -> usize {
    let mut escaped = false;
    for (offset, ch) in input[start + 1..].char_indices() {
        if escaped {
            escaped = false;
            continue;
        }

        match ch {
            '\\' => escaped = true,
            '"' => return start + 1 + offset + ch.len_utf8(),
            _ => {}
        }
    }

    input.len()
}

fn is_math_identifier_char(ch: char) -> bool {
    ch.is_alphanumeric() || matches!(ch, '.' | '\'')
}

static GREEK_LETTERS: &[&str] = &[
    "Alpha", "Beta", "Gamma", "Delta", "Epsilon", "Zeta", "Eta", "Theta", "Iota", "Kappa",
    "Lambda", "Mu", "Nu", "Xi", "Omicron", "Pi", "Rho", "Sigma", "Tau", "Upsilon", "Phi",
    "Chi", "Psi", "Omega", "alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta",
    "iota", "kappa", "lambda", "mu", "nu", "xi", "omicron", "pi", "rho", "sigma", "tau",
    "upsilon", "phi", "chi", "psi", "omega", "varepsilon", "varphi", "vartheta",
];

/// Replace `\cmd{a}{b}` with `cmd(a, b)`.
fn replace_command_two_args(input: &str, cmd: &str) -> String {
    let pattern = format!("\\{cmd}");
    let mut result = String::with_capacity(input.len());
    let mut remaining = input;

    while let Some(pos) = remaining.find(&pattern) {
        result.push_str(&remaining[..pos]);
        remaining = &remaining[pos + pattern.len()..];

        if let Some((arg1, rest)) = extract_braced_arg(remaining) {
            remaining = rest;
            if let Some((arg2, rest2)) = extract_braced_arg(remaining) {
                remaining = rest2;
                result.push_str(&format!("{cmd}({arg1}, {arg2})"));
            } else {
                // Only one arg found — restore original
                result.push_str(&format!("\\{cmd}{{{arg1}}}"));
            }
        } else {
            result.push_str(&format!("\\{cmd}"));
        }
    }
    result.push_str(remaining);
    result
}

/// Replace `\cmd{arg}` with `replacement(arg)` or just `arg` if replacement is empty.
fn replace_command_one_arg(input: &str, cmd: &str, replacement: &str) -> String {
    let pattern = format!("\\{cmd}");
    let mut result = String::with_capacity(input.len());
    let mut remaining = input;

    while let Some(pos) = remaining.find(&pattern) {
        result.push_str(&remaining[..pos]);
        remaining = &remaining[pos + pattern.len()..];

        if let Some((arg, rest)) = extract_braced_arg(remaining) {
            remaining = rest;
            if replacement.is_empty() {
                result.push_str(&arg);
            } else {
                result.push_str(&format!("{replacement}({arg})"));
            }
        } else {
            result.push_str(&format!("\\{cmd}"));
        }
    }
    result.push_str(remaining);
    result
}

/// Replace `\sqrt{x}` → `sqrt(x)` and `\sqrt[n]{x}` → `root(n, x)`.
fn replace_sqrt(input: &str) -> String {
    let pattern = "\\sqrt";
    let mut result = String::with_capacity(input.len());
    let mut remaining = input;

    while let Some(pos) = remaining.find(pattern) {
        result.push_str(&remaining[..pos]);
        remaining = &remaining[pos + pattern.len()..];

        // Check for optional [n] argument
        if remaining.starts_with('[') {
            if let Some(close) = remaining.find(']') {
                let n = &remaining[1..close];
                remaining = &remaining[close + 1..];
                if let Some((arg, rest)) = extract_braced_arg(remaining) {
                    remaining = rest;
                    result.push_str(&format!("root({n}, {arg})"));
                } else {
                    result.push_str(&format!("root({n}, )"));
                }
            } else {
                result.push_str("sqrt");
            }
        } else if let Some((arg, rest)) = extract_braced_arg(remaining) {
            remaining = rest;
            result.push_str(&format!("sqrt({arg})"));
        } else {
            result.push_str("sqrt");
        }
    }
    result.push_str(remaining);
    result
}

/// Replace `\begin{env}...\end{env}` matrix environments.
fn replace_matrix_env(input: &str, env: &str, delim: &str) -> String {
    let begin = format!("\\begin{{{env}}}");
    let end = format!("\\end{{{env}}}");
    let mut result = String::with_capacity(input.len());
    let mut remaining = input;

    while let Some(start_pos) = remaining.find(&begin) {
        result.push_str(&remaining[..start_pos]);
        remaining = &remaining[start_pos + begin.len()..];

        if let Some(end_pos) = remaining.find(&end) {
            let body = &remaining[..end_pos];
            remaining = &remaining[end_pos + end.len()..];

            // Convert LaTeX matrix body: `a & b \\ c & d` → `a, b; c, d`
            let typst_body = body
                .split("\\\\")
                .map(|row| row.split('&').map(str::trim).collect::<Vec<_>>().join(", "))
                .collect::<Vec<_>>()
                .join("; ");

            result.push_str(&format!("mat(delim: {delim}, {typst_body})"));
        } else {
            // No matching \end — leave as-is
            result.push_str(&begin);
        }
    }
    result.push_str(remaining);
    result
}

/// Extract `{content}` from the start of `input`, returning the content and the rest.
fn extract_braced_arg(input: &str) -> Option<(String, &str)> {
    let input = input.trim_start();
    if !input.starts_with('{') {
        return None;
    }

    let mut depth = 0i32;
    for (i, ch) in input.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    let content = &input[1..i];
                    let rest = &input[i + 1..];
                    return Some((content.to_string(), rest));
                }
            }
            _ => {}
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Minimal Typst World implementation
// ---------------------------------------------------------------------------

static MAIN_ID: LazyLock<FileId> =
    LazyLock::new(|| FileId::new_fake(VirtualPath::new("/main.typ")));

struct MathWorld {
    source: Source,
    library: LazyHash<Library>,
    book: LazyHash<FontBook>,
    fonts: Vec<Font>,
}

impl MathWorld {
    fn new(source: Source) -> Self {
        let fonts = &*EMBEDDED_FONTS;
        let book = LazyHash::new(FontBook::from_fonts(fonts));
        let library = LazyHash::new(Library::default());
        Self {
            source,
            library,
            book,
            fonts: fonts.clone(),
        }
    }
}

#[comemo::track]
impl World for MathWorld {
    fn library(&self) -> &LazyHash<Library> {
        &self.library
    }

    fn book(&self) -> &LazyHash<FontBook> {
        &self.book
    }

    fn main(&self) -> FileId {
        *MAIN_ID
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        if id == *MAIN_ID {
            Ok(self.source.clone())
        } else {
            Err(FileError::NotFound(id.vpath().as_rooted_path().into()))
        }
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        if id == *MAIN_ID {
            Ok(Bytes::new(self.source.text().as_bytes().to_vec()))
        } else {
            Err(FileError::NotFound(id.vpath().as_rooted_path().into()))
        }
    }

    fn font(&self, index: usize) -> Option<Font> {
        self.fonts.get(index).cloned()
    }

    fn today(&self, _offset: Option<i64>) -> Option<Datetime> {
        None
    }
}

// ---------------------------------------------------------------------------
// Font loading (done once via LazyLock)
// ---------------------------------------------------------------------------

static EMBEDDED_FONTS: LazyLock<Vec<Font>> = LazyLock::new(|| {
    let mut fonts = Vec::new();
    for data in typst_assets::fonts() {
        let bytes = Bytes::new(data);
        for font in Font::iter(bytes) {
            fonts.push(font);
        }
    }
    fonts
});

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn math_asset_prefix(requested_path: &Path, canonical_path: Option<&Path>) -> String {
    let identity = canonical_path.unwrap_or(requested_path);
    format!("math/{:016x}/", stable_hash(&identity.to_string_lossy()))
}

fn stable_hash(value: impl Hash) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preprocess_frac() {
        assert_eq!(preprocess_latex("\\frac{a}{b}"), "frac(a, b)");
    }

    #[test]
    fn preprocess_sqrt() {
        assert_eq!(preprocess_latex("\\sqrt{x}"), "sqrt(x)");
    }

    #[test]
    fn preprocess_sqrt_nth() {
        assert_eq!(preprocess_latex("\\sqrt[3]{x}"), "root(3, x)");
    }

    #[test]
    fn preprocess_greek() {
        // Greek letters get a leading space to avoid merging with adjacent identifiers.
        // Typst math ignores extra whitespace so this is safe.
        let result = preprocess_latex("\\alpha + \\beta");
        assert!(result.contains("alpha"), "expected alpha, got: {result}");
        assert!(result.contains("beta"), "expected beta, got: {result}");
    }

    #[test]
    fn preprocess_matrix() {
        let input = "\\begin{pmatrix} a & b \\\\ c & d \\end{pmatrix}";
        let result = preprocess_latex(input);
        assert!(result.contains("mat(delim:"), "result was: {}", result);
        assert!(result.contains("a, b"));
        assert!(result.contains("c, d"));
    }

    #[test]
    fn preprocess_operators() {
        assert!(preprocess_latex("\\infty").contains("infinity"));
        assert!(preprocess_latex("\\sum").contains("sum"));
        assert!(preprocess_latex("\\int").contains("integral"));
    }

    #[test]
    fn build_display_source() {
        let source = build_typst_source("x^2", true);
        assert!(source.contains("$ x^2 $"));
        assert!(source.contains("page(width: auto"));
    }

    #[test]
    fn build_inline_source() {
        let source = build_typst_source("x^2", false);
        assert!(source.contains("$x^2$"));
        assert!(source.contains("margin: (x: 0.08em, y: 0.03em)"));
        assert!(source.contains("text(size: 11pt"));
    }

    #[test]
    fn render_simple_formula() {
        let result = render_latex_to_png("x^2 + y^2 = z^2", true);
        assert!(result.is_ok(), "render failed: {:?}", result.err());
        let png = result.unwrap();
        // PNG files start with the signature 89 50 4E 47
        assert!(png.starts_with(&[0x89, 0x50, 0x4E, 0x47]), "Not a valid PNG");
    }

    #[test]
    fn render_fraction() {
        let result = render_latex_to_png("\\frac{a}{b}", true);
        assert!(result.is_ok(), "render failed: {:?}", result.err());
    }

    #[test]
    fn render_inline() {
        let result = render_latex_to_png("E = mc^2", false);
        assert!(result.is_ok(), "render failed: {:?}", result.err());
    }

    // -----------------------------------------------------------------------
    // preprocess_latex: hat / hbar / tensor / special symbols
    // -----------------------------------------------------------------------

    #[test]
    fn preprocess_hat_notation() {
        // \hat{H} should become hat(H) in Typst
        let result = preprocess_latex("\\hat{H}");
        assert!(result.contains("hat(H)"), "expected hat(H), got: {result}");
    }

    #[test]
    fn preprocess_hat_f_xi() {
        // \hat{f}(\xi)  — Fourier transform notation
        let result = preprocess_latex("\\hat{f}(\\xi)");
        assert!(result.contains("hat("), "expected hat(...), got: {result}");
        assert!(result.contains("xi"), "expected xi, got: {result}");
    }

    #[test]
    fn preprocess_hbar() {
        // \hbar → ℏ  (Unicode reduced Planck constant)
        let result = preprocess_latex("E = \\hbar \\omega");
        assert!(result.contains('ℏ'), "expected ℏ (Unicode), got: {result}");
    }

    #[test]
    fn preprocess_widehat() {
        // \widehat{f} → hat(f)  (wide hat variant)
        let result = preprocess_latex("\\widehat{f}");
        assert!(result.contains("hat(f)"), "expected hat(f), got: {result}");
    }

    #[test]
    fn preprocess_varepsilon() {
        // \varepsilon → ε  (Unicode, since Typst doesn't have "varepsilon")
        let result = preprocess_latex("\\varepsilon_0");
        assert!(result.contains('ε'), "expected ε (Unicode), got: {result}");
    }

    #[test]
    fn preprocess_mu_nu_tensor() {
        // T^{\mu}_{\nu} — tensor notation
        let result = preprocess_latex("T^{\\mu}_{\\nu}");
        assert_eq!(result, "attach(T, b: nu, t: mu)");
    }

    #[test]
    fn preprocess_euler_identity() {
        // e^{i\pi} + 1 = 0
        let result = preprocess_latex("e^{i\\pi} + 1 = 0");
        assert_eq!(result, "attach(e, t: i pi) + 1 = 0");
    }

    #[test]
    fn preprocess_hamiltonian_hat() {
        // \hat{H}\psi = E\psi
        let result = preprocess_latex("\\hat{H}\\psi = E\\psi");
        assert!(result.contains("hat(H)"), "expected hat(H) in hamiltonian, got: {result}");
        assert!(result.contains("psi"), "expected psi, got: {result}");
    }

    #[test]
    fn preprocess_maxwell_nabla_dot_e() {
        // \nabla \cdot \mathbf{E} = \frac{\rho}{\varepsilon_0}
        let result = preprocess_latex("\\nabla \\cdot \\mathbf{E} = \\frac{\\rho}{\\varepsilon_0}");
        assert!(result.contains("nabla"), "expected nabla, got: {result}");
        assert!(result.contains("bold(E)"), "expected bold(E) for mathbf{{E}}, got: {result}");
        assert!(result.contains("frac("), "expected frac, got: {result}");
        assert!(result.contains('ε'), "expected ε (Unicode) for varepsilon, got: {result}");
    }

    #[test]
    fn preprocess_fourier_transform_full() {
        // \hat{f}(\xi) = \int_{-\infty}^{\infty} f(x) e^{-2\pi i x \xi} dx
        let result = preprocess_latex("\\hat{f}(\\xi) = \\int_{-\\infty}^{\\infty} f(x) e^{-2\\pi i x \\xi} dx");
        assert!(result.contains("hat("), "expected hat for Fourier, got: {result}");
        assert!(result.contains("attach(integral, b: -infinity, t: infinity)"), "expected integral limits attach, got: {result}");
        assert!(result.contains("infinity"), "expected infinity, got: {result}");
        assert!(result.contains("pi"), "expected pi, got: {result}");
        assert!(!result.contains('{') && !result.contains('}'), "grouping braces should be removed: {result}");
    }

    #[test]
    fn preprocess_gaussian_integral() {
        // \int_{-\infty}^{\infty} e^{-x^2} dx = \sqrt{\pi}
        let result = preprocess_latex("\\int_{-\\infty}^{\\infty} e^{-x^2} dx = \\sqrt{\\pi}");
        assert!(result.contains("attach(integral, b: -infinity, t: infinity)"), "expected integral limits attach, got: {result}");
        assert!(result.contains("infinity"), "expected infinity, got: {result}");
        assert!(result.contains("attach(e, t: -x^2)"), "expected grouped exponent attach, got: {result}");
        assert!(result.contains("sqrt("), "expected sqrt, got: {result}");
        assert!(result.contains("pi"), "expected pi, got: {result}");
        assert!(!result.contains('{') && !result.contains('}'), "grouping braces should be removed: {result}");
    }

    #[test]
    fn preprocess_sum_with_limits() {
        // \sum_{i=1}^{n} x_i
        let result = preprocess_latex("\\sum_{i=1}^{n} x_i");
        assert_eq!(result, "attach(sum, b: i=1, t: n) x_i");
    }

    #[test]
    fn preprocess_bold_symbol_with_grouped_subscript() {
        let result = preprocess_latex("\\mathbf{E}_{0}");
        assert_eq!(result, "attach(bold(E), b: 0)");
    }

    #[test]
    fn preprocess_planck_energy_formula() {
        // E = \hbar \omega  → ℏ (Unicode) omega
        let result = preprocess_latex("E = \\hbar \\omega");
        assert!(result.contains('ℏ'), "expected ℏ (Unicode), got: {result}");
        assert!(result.contains("omega"), "expected omega, got: {result}");
    }

    #[test]
    fn preprocess_quadratic_formula() {
        // x = \frac{-b \pm \sqrt{b^2 - 4ac}}{2a}
        let result = preprocess_latex("x = \\frac{-b \\pm \\sqrt{b^2 - 4ac}}{2a}");
        assert!(result.contains("frac("), "expected frac, got: {result}");
        assert!(result.contains("plus.minus"), "expected plus.minus for \\pm, got: {result}");
        assert!(result.contains("sqrt("), "expected sqrt, got: {result}");
    }

    // -----------------------------------------------------------------------
    // render_latex_to_png: specific physics formulas
    // -----------------------------------------------------------------------

    #[test]
    fn render_euler_identity_display() {
        let result = render_latex_to_png("e^{i\\pi} + 1 = 0", true);
        assert!(result.is_ok(), "Euler identity render failed: {:?}", result.err());
        let png = result.unwrap();
        assert!(png.starts_with(&[0x89, 0x50, 0x4E, 0x47]), "expected valid PNG");
        // Should produce a reasonably sized image
        assert!(png.len() > 100, "PNG too small: {} bytes", png.len());
    }

    #[test]
    fn render_hamiltonian_display() {
        let result = render_latex_to_png("\\hat{H}\\psi = E\\psi", true);
        assert!(result.is_ok(), "Hamiltonian render failed: {:?}", result.err());
        let png = result.unwrap();
        assert!(png.starts_with(&[0x89, 0x50, 0x4E, 0x47]), "expected valid PNG");
    }

    #[test]
    fn render_maxwell_first_equation() {
        let result = render_latex_to_png("\\nabla \\cdot \\mathbf{E} = \\frac{\\rho}{\\varepsilon_0}", true);
        assert!(result.is_ok(), "Maxwell eq render failed: {:?}", result.err());
        let png = result.unwrap();
        assert!(png.starts_with(&[0x89, 0x50, 0x4E, 0x47]), "expected valid PNG");
    }

    #[test]
    fn render_fourier_transform() {
        let result = render_latex_to_png(
            "\\hat{f}(\\xi) = \\int_{-\\infty}^{\\infty} f(x) e^{-2\\pi i x \\xi} dx",
            true
        );
        assert!(result.is_ok(), "Fourier transform render failed: {:?}", result.err());
    }

    #[test]
    fn render_gaussian_integral() {
        let result = render_latex_to_png("\\int_{-\\infty}^{\\infty} e^{-x^2} dx = \\sqrt{\\pi}", true);
        assert!(result.is_ok(), "Gaussian integral render failed: {:?}", result.err());
    }

    #[test]
    fn render_planck_constant() {
        let result = render_latex_to_png("E = \\hbar \\omega", true);
        assert!(result.is_ok(), "Planck constant render failed: {:?}", result.err());
    }

    #[test]
    fn render_tensor_notation() {
        let result = render_latex_to_png("T^{\\mu}_{\\nu}", true);
        assert!(result.is_ok(), "tensor notation render failed: {:?}", result.err());
    }

    #[test]
    fn render_sum_notation_display() {
        let result = render_latex_to_png("\\sum_{i=1}^{n} x_i", true);
        assert!(result.is_ok(), "sum notation render failed: {:?}", result.err());
    }

    #[test]
    fn render_quadratic_formula() {
        let result = render_latex_to_png("x = \\frac{-b \\pm \\sqrt{b^2 - 4ac}}{2a}", true);
        assert!(result.is_ok(), "quadratic formula render failed: {:?}", result.err());
    }

    #[test]
    fn render_mass_energy_inline() {
        // E = mc^2 as inline math
        let result = render_latex_to_png("E = mc^2", false);
        assert!(result.is_ok(), "inline E=mc^2 render failed: {:?}", result.err());
        let png = result.unwrap();
        assert!(png.starts_with(&[0x89, 0x50, 0x4E, 0x47]), "expected valid PNG");
    }

    #[test]
    fn render_maxwell_curl_b() {
        let result = render_latex_to_png(
            "\\nabla \\times \\mathbf{B} = \\mu_0 \\mathbf{J} + \\mu_0 \\varepsilon_0 \\frac{\\partial \\mathbf{E}}{\\partial t}",
            true
        );
        assert!(result.is_ok(), "Maxwell curl B render failed: {:?}", result.err());
    }

    // -----------------------------------------------------------------------
    // PNG dimension checks: display math should be larger than inline
    // -----------------------------------------------------------------------

    #[test]
    fn display_math_png_dimensions_reasonable() {
        let png = render_latex_to_png("x^2 + y^2 = r^2", true).expect("render failed");
        // Check PNG dimensions from IHDR chunk (bytes 16-23)
        assert!(png.len() > 24, "PNG too small");
        let width = u32::from_be_bytes([png[16], png[17], png[18], png[19]]);
        let height = u32::from_be_bytes([png[20], png[21], png[22], png[23]]);
        assert!(width > 0 && width < 2000, "unexpected width: {width}");
        assert!(height > 0 && height < 500, "unexpected height: {height}");
    }

    #[test]
    fn inline_math_png_dimensions_smaller_than_display() {
        let display_png = render_latex_to_png("E = mc^2", true).expect("display render failed");
        let inline_png = render_latex_to_png("E = mc^2", false).expect("inline render failed");
        // Extract heights from IHDR
        let display_height = u32::from_be_bytes([display_png[20], display_png[21], display_png[22], display_png[23]]);
        let inline_height = u32::from_be_bytes([inline_png[20], inline_png[21], inline_png[22], inline_png[23]]);
        assert!(
            inline_height <= display_height,
            "inline PNG ({inline_height}px) should be no taller than display PNG ({display_height}px)"
        );
    }

    #[test]
    fn different_formulas_produce_different_pngs() {
        let png1 = render_latex_to_png("x^2", true).expect("render 1 failed");
        let png2 = render_latex_to_png("x^3", true).expect("render 2 failed");
        assert_ne!(png1, png2, "different formulas should produce different PNGs");
    }

    #[test]
    fn same_formula_same_png() {
        let png1 = render_latex_to_png("E = mc^2", true).expect("render 1 failed");
        let png2 = render_latex_to_png("E = mc^2", true).expect("render 2 failed");
        assert_eq!(png1, png2, "same formula should produce identical PNGs");
    }
}
