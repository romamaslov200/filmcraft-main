//! Dynamic Link model: a live reference to a comp/sequence in another Craft app,
//! resolved over that app's control channel (TCP loopback JSON-lines).
//!
//! This file format (`.craftlink`) is shared between EffectCraft, FilmCraft and PhotoCraft.
//! Each app stores the link as its own project item/media kind.

use serde::{Deserialize, Serialize};

/// Which Craft app the source project lives in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkApp {
    #[default]
    EffectCraft,
    FilmCraft,
    PhotoCraft,
}

impl LinkApp {
    pub fn id(self) -> &'static str {
        match self {
            LinkApp::EffectCraft => "effectcraft",
            LinkApp::FilmCraft => "filmcraft",
            LinkApp::PhotoCraft => "photocraft",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            LinkApp::EffectCraft => "EffectCraft",
            LinkApp::FilmCraft => "FilmCraft",
            LinkApp::PhotoCraft => "PhotoCraft",
        }
    }
}

/// One linkable item inside a source project.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkItem {
    pub id: u64,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub frame_rate_num: u32,
    pub frame_rate_den: u32,
    pub duration_seconds: f64,
}

/// What a link points at: the app, its project file, and the item within it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkTarget {
    pub app: LinkApp,
    pub project: String,
    pub item: String,
    pub port: u16,
    /// The source app's own ID for the item, when the app that made the link knew it. The name
    /// alone is ambiguous — two compositions can share one — and a link resolving to the wrong
    /// item shows the wrong picture in the preview.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_rate_num: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_rate_den: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<f64>,
}

impl LinkTarget {
    pub fn new(app: LinkApp, project: String, item: String, port: u16) -> Self {
        Self { app, project, item, item_id: None, port, width: None, height: None, frame_rate_num: None, frame_rate_den: None, duration_seconds: None }
    }
}

/// The on-disk `.craftlink` file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CraftLinkFile {
    pub schema: u32,
    pub target: LinkTarget,
}

const SCHEMA: u32 = 1;

impl CraftLinkFile {
    pub fn new(target: LinkTarget) -> Self {
        Self { schema: SCHEMA, target }
    }

    pub fn to_json(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }

    pub fn from_json(text: &str) -> serde_json::Result<Self> {
        let v: Self = serde_json::from_str(text)?;
        if v.schema > SCHEMA {
            return Err(serde::de::Error::custom(format!(
                "unsupported .craftlink schema {} (this build supports {})",
                v.schema, SCHEMA
            )));
        }
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_json() {
        let file = CraftLinkFile::new(LinkTarget {
            app: LinkApp::EffectCraft,
            project: "C:/Projects/Show.ecproj".into(),
            item: "Hero Comp".into(),
            port: 9877,
            ..Default::default()
        });
        let text = file.to_json().unwrap();
        let back = CraftLinkFile::from_json(&text).unwrap();
        assert_eq!(back, file);
    }
}
