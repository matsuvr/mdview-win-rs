use std::{
    hash::{Hash, Hasher},
    path::Path,
};

use mermaid_rs_renderer::render;

use crate::{
    assets::AppAssets,
    markdown::{Block, MarkdownDocument, MermaidRender},
};

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
                    "{prefix}diagram-{index:04}-{hash:016x}.svg",
                    index = *block_index,
                    hash = stable_hash(&diagram.code),
                );

                match render(&diagram.code) {
                    Ok(svg) => {
                        assets.insert_svg(asset_path.clone(), svg);
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
            | Block::Rule
            | Block::Table(_) => {}
        }
    }
}

fn document_asset_prefix(requested_path: &Path, canonical_path: Option<&Path>) -> String {
    let identity = canonical_path.unwrap_or(requested_path);
    format!("mermaid/{:016x}/", stable_hash(&identity.to_string_lossy()))
}

fn stable_hash(value: impl Hash) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}
