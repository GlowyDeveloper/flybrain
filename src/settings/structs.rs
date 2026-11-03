use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(Default)]
pub struct EnvToml {
    pub visual: EnvVisual,
    pub shared: EnvShared,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvVisual {
    pub temp: String,
}

impl Default for EnvVisual {
    fn default() -> Self {
        EnvVisual {
            temp: "temp".to_string(),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvShared {
    pub lane_1: String,
    pub lane_2: String,
}

impl Default for EnvShared {
    fn default() -> Self {
        EnvShared {
            lane_1: "default".to_string(),
            lane_2: "1".to_string(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(deny_unknown_fields)]
#[derive(Default)]
pub struct SaveToml {
    pub application: SaveApplication,
    pub options: SaveOptions,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(deny_unknown_fields)]
#[derive(Default)]
pub struct SaveApplication {
    pub env: Vec<(String, String)>,
    pub args: Vec<String>,
    pub executable: String,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(deny_unknown_fields)]
pub struct SaveOptions {
    pub width: i32,
    pub height: i32,
    pub hz: i32,
    pub window: Option<String>,
    pub fullscreen: bool,
    pub quiet: bool,
}

impl Default for SaveOptions {
    fn default() -> Self {
        SaveOptions {
            width: 1280,
            height: 720,
            hz: 60,
            window: None,
            fullscreen: false,
            quiet: false,
        }
    }
}
