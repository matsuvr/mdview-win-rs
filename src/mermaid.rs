use std::{
    hash::{Hash, Hasher},
    path::Path,
    sync::{Arc, OnceLock},
};

use fontdb::Database;
use mermaid_rs_renderer::render;
use resvg::tiny_skia::Pixmap;
use resvg::usvg::{Options, Tree};

use crate::{
    assets::AppAssets,
    markdown::{Block, MarkdownDocument, MermaidRender},
};

/// Global font database loaded once at first use.
/// Loads fonts from Windows system directories for multilingual support.
static FONTDB: OnceLock<Arc<Database>> = OnceLock::new();

/// Get or initialize the font database.
fn get_fontdb() -> Arc<Database> {
    FONTDB.get_or_init(|| {
        let mut db = Database::new();

        // Load system fonts from Windows Fonts directory
        // This is the standard location for all Windows system fonts
        let system_fonts = Path::new("C:\\Windows\\Fonts");
        if system_fonts.exists() {
            db.load_fonts_dir(system_fonts);
        }

        // Load user-installed fonts (Windows 10 1809+)
        // These are per-user fonts installed via Settings > Personalization > Fonts
        if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
            let user_fonts = Path::new(&local_app_data).join("Microsoft\\Windows\\Fonts");
            if user_fonts.exists() {
                db.load_fonts_dir(&user_fonts);
            }
        }

        Arc::new(db)
    }).clone()
}

pub fn hydrate_mermaid_blocks(
    document: &mut MarkdownDocument,
    requested_path: &Path,
    canonical_path: Option<&Path>,
    assets: &AppAssets,
) -> String {
    let prefix = document_asset_prefix(requested_path, canonical_path);
    assets.remove_prefix(&prefix);

    let mut block_index = 0usize;
    hydrate_blocks(&mut document.blocks, &prefix, assets, &mut block_index);
    prefix
}

fn hydrate_blocks(blocks: &mut [Block], prefix: &str, assets: &AppAssets, block_index: &mut usize) {
    for block in blocks {
        match block {
            Block::BlockQuote(children) => hydrate_blocks(children, prefix, assets, block_index),
            Block::List(list) => {
                for item in &mut list.items {
                    hydrate_blocks(&mut item.blocks, prefix, assets, block_index);
                }
            }
            Block::Mermaid(diagram) => {
                let asset_path = format!(
                    "{prefix}diagram-{index:04}-{hash:016x}.png",
                    index = *block_index,
                    hash = stable_hash(&diagram.code),
                );

                match render_and_convert_to_png(&diagram.code) {
                    Ok(png_data) => {
                        assets.insert_bytes(asset_path.clone(), png_data);
                        diagram.render = MermaidRender::Rendered { asset_path };
                    }
                    Err(error) => {
                        diagram.render = MermaidRender::Failed {
                            message: error.to_string(),
                        };
                    }
                }

                *block_index += 1;
            }
            Block::Heading { .. }
            | Block::Paragraph(_)
            | Block::CodeBlock { .. }
            | Block::Math(_)
            | Block::Rule
            | Block::Table(_) => {}
        }
    }
}

/// Render mermaid diagram to SVG, then convert to PNG
fn render_and_convert_to_png(code: &str) -> Result<Vec<u8>, String> {
    // Step 1: Render to SVG using mermaid-rs-renderer
    let svg = render(code).map_err(|e| format!("Mermaid render error: {}", e))?;

    // Step 2: Fix malformed SVG - mermaid-rs-renderer produces invalid XML
    // with unescaped quotes inside attribute values like:
    // font-family="Inter, ..., "Segoe UI", sans-serif"
    // We need to escape the inner quotes
    let svg = fix_svg_quotes(&svg);

    // Step 3: Parse SVG with usvg, using our font database
    let fontdb = get_fontdb();
    let mut options = Options::default();
    options.fontdb = fontdb;

    let tree = Tree::from_str(&svg, &options)
        .map_err(|e| format!("Failed to parse SVG: {}", e))?;

    // Step 4: Get dimensions and create pixmap
    let size = tree.size();
    let width = size.width() as u32;
    let height = size.height() as u32;

    // Ensure minimum size
    let width = width.max(1);
    let height = height.max(1);

    let mut pixmap = Pixmap::new(width, height)
        .ok_or_else(|| "Failed to create pixmap".to_string())?;

    // Step 5: Render SVG to pixmap
    resvg::render(&tree, resvg::tiny_skia::Transform::identity(), &mut pixmap.as_mut());

    // Step 6: Encode to PNG
    let png_data = pixmap.encode_png()
        .map_err(|e| format!("Failed to encode PNG: {}", e))?;

    Ok(png_data)
}

fn document_asset_prefix(requested_path: &Path, canonical_path: Option<&Path>) -> String {
    let identity = canonical_path.unwrap_or(requested_path);
    format!("mermaid/{:016x}/", stable_hash(&identity.to_string_lossy()))
}

/// Fix malformed SVG from mermaid-rs-renderer which has unescaped quotes inside attribute values.
///
/// The mermaid-rs-renderer produces invalid XML like:
/// `font-family="Inter, ui-sans-serif, system-ui, -apple-system, "Segoe UI", sans-serif"`
///
/// The inner `"Segoe UI"` quotes need to be escaped as `&quot;Segoe UI&quot;`
fn fix_svg_quotes(svg: &str) -> String {
    // Strategy: Find attribute value patterns like ="..." and escape inner quotes.
    // The challenge is identifying which quotes are "real" end quotes vs inner quotes.
    //
    // A quote is likely the END of an attribute if it's followed by:
    // 1. Whitespace + attribute name + "=" (e.g., `" other="`)
    // 2. `>` or `/>` (end of tag)
    // 3. End of string
    //
    // Otherwise, it's an inner quote that needs escaping.

    let mut result = String::with_capacity(svg.len() + svg.len() / 10);
    let chars: Vec<char> = svg.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let ch = chars[i];

        // Look for attribute start pattern: =" or ='
        if ch == '=' && i + 1 < chars.len() && (chars[i + 1] == '"' || chars[i + 1] == '\'') {
            let quote_char = chars[i + 1];
            result.push('=');
            result.push(quote_char);
            i += 2;

            // Process attribute value until we find the "real" end quote
            while i < chars.len() {
                let c = chars[i];

                if c == quote_char {
                    // Is this the real end quote?
                    let is_end = is_attribute_end(&chars, i + 1);

                    if is_end {
                        result.push(quote_char);
                        i += 1;
                        break;
                    } else {
                        // Inner quote - escape it
                        result.push_str("&quot;");
                        i += 1;
                    }
                } else {
                    result.push(c);
                    i += 1;
                }
            }
        } else {
            result.push(ch);
            i += 1;
        }
    }

    result
}

/// Check if the position after a quote looks like the end of an attribute value.
fn is_attribute_end(chars: &[char], pos: usize) -> bool {
    // Skip whitespace
    let mut j = pos;
    while j < chars.len() && (chars[j] == ' ' || chars[j] == '\t' || chars[j] == '\n' || chars[j] == '\r') {
        j += 1;
    }

    // End of string = this is the end
    if j >= chars.len() {
        return true;
    }

    let next = chars[j];

    // End of tag
    if next == '>' {
        return true;
    }

    // Self-closing tag
    if next == '/' && j + 1 < chars.len() && chars[j + 1] == '>' {
        return true;
    }

    // New attribute starting: whitespace + attribute_name + "="
    // Attribute names start with a letter or underscore
    if next == '_' || next.is_ascii_alphabetic() {
        // Look ahead for "=" after the attribute name
        let mut k = j;
        while k < chars.len() && (chars[k] == '_' || chars[k].is_ascii_alphanumeric() || chars[k] == '-' || chars[k] == ':') {
            k += 1;
        }
        // Skip whitespace before =
        while k < chars.len() && (chars[k] == ' ' || chars[k] == '\t') {
            k += 1;
        }
        if k < chars.len() && chars[k] == '=' {
            return true;
        }
    }

    false
}

fn stable_hash(value: impl Hash) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // SVG quote fixing
    // -----------------------------------------------------------------------

    #[test]
    fn test_fix_svg_quotes() {
        let input = r#"font-family="Inter, "Segoe UI", sans-serif""#;
        let expected = r#"font-family="Inter, &quot;Segoe UI&quot;, sans-serif""#;
        let result = fix_svg_quotes(input);
        assert_eq!(result, expected, "Got: {}", result);
    }

    #[test]
    fn test_fix_svg_quotes_multiple() {
        let input = r#"font-family="Inter, "Segoe UI", "SF Pro", sans-serif" other="value""#;
        let expected = r#"font-family="Inter, &quot;Segoe UI&quot;, &quot;SF Pro&quot;, sans-serif" other="value""#;
        let result = fix_svg_quotes(input);
        assert_eq!(result, expected, "Got: {}", result);
    }

    #[test]
    fn test_fix_svg_quotes_end_of_tag() {
        let input = r#"font-family="Inter, "Segoe UI", sans-serif">"#;
        let expected = r#"font-family="Inter, &quot;Segoe UI&quot;, sans-serif">"#;
        let result = fix_svg_quotes(input);
        assert_eq!(result, expected, "Got: {}", result);
    }

    #[test]
    fn test_fix_svg_quotes_clean_svg_unchanged() {
        let input = r#"font-family="Arial, sans-serif" fill="black""#;
        let result = fix_svg_quotes(input);
        assert_eq!(result, input, "clean SVG should be unchanged: {}", result);
    }

    #[test]
    fn test_fix_svg_self_closing_tag() {
        let input = r#"<path d="M0 0" font-family="Inter, "Segoe UI", sans"/>"#;
        let result = fix_svg_quotes(input);
        assert!(result.contains("&quot;Segoe UI&quot;"), "expected escaped inner quotes: {result}");
    }

    // -----------------------------------------------------------------------
    // Basic render coverage
    // -----------------------------------------------------------------------

    #[test]
    fn test_render_simple_flowchart() {
        let code = "graph TD\n    A[Hello] --> B[World]";
        let result = render_and_convert_to_png(code);
        assert!(result.is_ok(), "render failed: {:?}", result.err());
        let png = result.unwrap();
        assert!(png.starts_with(&[0x89, 0x50, 0x4E, 0x47]), "Not a valid PNG");
    }

    #[test]
    fn test_render_sequence_diagram() {
        let code = "sequenceDiagram\n    Alice->>Bob: Hello\n    Bob-->>Alice: Hi!";
        let result = render_and_convert_to_png(code);
        assert!(result.is_ok(), "render failed: {:?}", result.err());
    }

    // -----------------------------------------------------------------------
    // Diagram type coverage
    // -----------------------------------------------------------------------

    #[test]
    fn test_render_class_diagram() {
        let code = "classDiagram\n    class Document {\n        +String path\n        +parse()\n    }\n    class Viewer {\n        +render()\n    }";
        let result = render_and_convert_to_png(code);
        assert!(result.is_ok(), "class diagram render failed: {:?}", result.err());
        let png = result.unwrap();
        assert!(png.starts_with(&[0x89, 0x50, 0x4E, 0x47]), "expected valid PNG");
    }

    #[test]
    fn test_render_readme_flowchart() {
        // Exact flowchart from README.md
        let code = "flowchart TD\n    A[ファイルを開く] --> B{ファイル存在?}\n    B -->|はい| C[Markdown パース]\n    B -->|いいえ| D[エラー表示]\n    C --> E[Mermaid/数式 変換]\n    E --> F[GPUI で描画]";
        let result = render_and_convert_to_png(code);
        assert!(result.is_ok(), "README flowchart render failed: {:?}", result.err());
        let png = result.unwrap();
        assert!(png.starts_with(&[0x89, 0x50, 0x4E, 0x47]), "expected valid PNG");
    }

    #[test]
    fn test_render_readme_sequence_diagram() {
        // Exact sequence diagram from README.md
        let code = "sequenceDiagram\n    participant User\n    participant mdview\n    participant Parser\n    participant Renderer\n\n    User->>mdview: ファイルを開く\n    mdview->>Parser: テキストを渡す\n    Parser->>Parser: Markdown 解析\n    Parser->>Renderer: AST を渡す\n    Renderer->>mdview: 描画完了\n    mdview->>User: ウインドウ表示";
        let result = render_and_convert_to_png(code);
        assert!(result.is_ok(), "README sequence diagram render failed: {:?}", result.err());
    }

    #[test]
    fn test_render_readme_class_diagram() {
        // Exact class diagram from README.md
        let code = "classDiagram\n    class Document {\n        +String path\n        +String content\n        +parse()\n    }\n    class Viewer {\n        +render()\n        +update()\n    }\n    class Registry {\n        +Map~String, Document~ docs\n        +register()\n        +focus()\n    }\n    Registry --> Document\n    Viewer --> Document";
        let result = render_and_convert_to_png(code);
        assert!(result.is_ok(), "README class diagram render failed: {:?}", result.err());
    }

    #[test]
    fn test_render_simple_graph_from_test_mermaid_md() {
        // Exact content from test_mermaid.md
        let code = "graph TD\n    A[Hello] --> B[World]";
        let result = render_and_convert_to_png(code);
        assert!(result.is_ok(), "test_mermaid.md graph render failed: {:?}", result.err());
        let png = result.unwrap();
        assert!(png.starts_with(&[0x89, 0x50, 0x4E, 0x47]), "expected valid PNG");
    }

    // -----------------------------------------------------------------------
    // PNG dimension / quality checks
    // -----------------------------------------------------------------------

    #[test]
    fn test_rendered_png_has_non_zero_dimensions() {
        let code = "graph TD\n    A --> B";
        let png = render_and_convert_to_png(code).expect("render failed");
        assert!(png.len() > 24, "PNG too small to have IHDR");
        let width = u32::from_be_bytes([png[16], png[17], png[18], png[19]]);
        let height = u32::from_be_bytes([png[20], png[21], png[22], png[23]]);
        assert!(width > 0, "PNG width should be non-zero");
        assert!(height > 0, "PNG height should be non-zero");
    }

    #[test]
    fn test_rendered_png_dimensions_within_bounds() {
        let code = "graph TD\n    A --> B --> C --> D";
        let png = render_and_convert_to_png(code).expect("render failed");
        let width = u32::from_be_bytes([png[16], png[17], png[18], png[19]]);
        let height = u32::from_be_bytes([png[20], png[21], png[22], png[23]]);
        assert!(width < 4000, "diagram width too large: {width}");
        assert!(height < 4000, "diagram height too large: {height}");
        assert!(width > 10, "diagram width suspiciously small: {width}");
        assert!(height > 10, "diagram height suspiciously small: {height}");
    }

    #[test]
    fn test_different_diagrams_produce_different_pngs() {
        let code1 = "graph TD\n    A --> B";
        let code2 = "graph TD\n    A --> B --> C --> D --> E";
        let png1 = render_and_convert_to_png(code1).expect("render 1 failed");
        let png2 = render_and_convert_to_png(code2).expect("render 2 failed");
        assert_ne!(png1.len(), png2.len(),
            "different diagrams should produce different sized PNGs");
    }

    #[test]
    fn test_same_diagram_produces_same_png() {
        let code = "graph TD\n    A[Hello] --> B[World]";
        let png1 = render_and_convert_to_png(code).expect("render 1 failed");
        let png2 = render_and_convert_to_png(code).expect("render 2 failed");
        assert_eq!(png1, png2, "same diagram should produce identical PNGs");
    }

    #[test]
    fn test_complex_flowchart_renders_completely() {
        let code = "flowchart TD\n    Start([Start]) --> Parse[Parse Markdown]\n    Parse --> HasMath{Contains Math?}\n    HasMath -->|Yes| RenderMath[Render with Typst]\n    HasMath -->|No| Continue\n    RenderMath --> Continue[Continue]\n    Continue --> HasMermaid{Contains Mermaid?}\n    HasMermaid -->|Yes| RenderMermaid[Render Mermaid]\n    HasMermaid -->|No| Display\n    RenderMermaid --> Display[Display in GPUI]\n    Display --> End([End])";
        let result = render_and_convert_to_png(code);
        assert!(result.is_ok(), "complex flowchart render failed: {:?}", result.err());
        let png = result.unwrap();
        let height = u32::from_be_bytes([png[20], png[21], png[22], png[23]]);
        assert!(height > 50, "flowchart with many nodes should have height > 50px, got {height}");
    }
}
