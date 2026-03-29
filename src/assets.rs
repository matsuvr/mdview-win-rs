use std::{
    borrow::Cow,
    collections::HashMap,
    sync::{Arc, RwLock},
};

use anyhow::Result;
use gpui::{AssetSource, Global, SharedString};

#[derive(Clone, Default)]
pub struct AppAssets {
    inner: Arc<RwLock<HashMap<String, Vec<u8>>>>,
}

impl AppAssets {
    pub fn insert_bytes(&self, path: impl Into<String>, bytes: Vec<u8>) -> String {
        let path = normalize_asset_path(path.into());
        let mut guard = write_map(&self.inner);
        guard.insert(path.clone(), bytes);
        path
    }

    pub fn remove_prefix(&self, prefix: &str) {
        let prefix = normalize_asset_path(prefix.to_string());
        let mut guard = write_map(&self.inner);
        guard.retain(|path, _| !path.starts_with(&prefix));
    }
}

impl Global for AppAssets {}

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if path.is_empty() {
            return Ok(None);
        }

        let normalized_path = normalize_asset_path(path.to_string());

        let guard = read_map(&self.inner);
        let result: Option<Cow<'static, [u8]>> = guard.get(&normalized_path).cloned().map(Cow::Owned);

        Ok(result)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let prefix = normalize_asset_path(path.to_string());
        let guard = read_map(&self.inner);
        let mut entries: Vec<SharedString> = guard
            .keys()
            .filter(|entry| prefix.is_empty() || entry.starts_with(&prefix))
            .cloned()
            .map(SharedString::from)
            .collect();
        entries.sort();
        Ok(entries)
    }
}

fn normalize_asset_path(path: String) -> String {
    path.replace('\\', "/").trim_start_matches('/').to_string()
}

fn read_map(
    inner: &RwLock<HashMap<String, Vec<u8>>>,
) -> std::sync::RwLockReadGuard<'_, HashMap<String, Vec<u8>>> {
    match inner.read() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn write_map(
    inner: &RwLock<HashMap<String, Vec<u8>>>,
) -> std::sync::RwLockWriteGuard<'_, HashMap<String, Vec<u8>>> {
    match inner.write() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}
