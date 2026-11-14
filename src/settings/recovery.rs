use crate::settings::structs::{
    EnvLearning, EnvShared, EnvToml, EnvVisual, SaveApplication, SaveOptions, SaveToml,
};
use toml::Value;
use tracing::error;

pub fn fix_env_toml(broken_content: &str) -> EnvToml {
    let raw_value: Value = match toml::from_str(broken_content) {
        Ok(val) => val,
        Err(_) => {
            error!("Unable to parse env.toml");
            return EnvToml::default();
        }
    };

    let mut default = EnvToml::default();

    if let Some(visual) = raw_value.get("visual") {
        default.visual = fix_visual(visual);
    }

    if let Some(shared) = raw_value.get("shared") {
        default.shared = fix_shared(shared);
    }

    if let Some(learning) = raw_value.get("learning") {
        default.learning = fix_learning(learning);
    }

    default
}

pub fn fix_visual(broken: &Value) -> EnvVisual {
    let mut default = EnvVisual::default();

    if let Some(temp) = broken.get("temp").and_then(|v| v.as_str()) {
        default.temp = temp.to_string();
    }

    default
}

pub fn fix_shared(broken: &Value) -> EnvShared {
    let mut default = EnvShared::default();

    if let Some(lane_1) = broken.get("lane_1").and_then(|v| v.as_str()) {
        default.lane_1 = lane_1.to_string();
    }
    if let Some(lane_2) = broken.get("lane_2").and_then(|v| v.as_str()) {
        default.lane_2 = lane_2.to_string();
    }

    default
}

pub fn fix_learning(broken: &Value) -> EnvLearning {
    let mut default = EnvLearning::default();

    if let Some(learning_rate) = broken.get("learning_rate").and_then(|v| v.as_float()) {
        default.learning_rate = learning_rate as f32;
    }
    if let Some(trace_delay) = broken.get("trace_delay").and_then(|v| v.as_float()) {
        default.trace_delay = trace_delay as f32;
    }

    default
}

pub fn fix_save_toml(broken_content: &str) -> SaveToml {
    let raw_value: Value = match toml::from_str(broken_content) {
        Ok(val) => val,
        Err(_) => {
            error!("Unable to parse toml");
            return SaveToml::default();
        }
    };

    let mut default = SaveToml::default();

    if let Some(application) = raw_value.get("application") {
        default.application = fix_application(application);
    }

    if let Some(options) = raw_value.get("options") {
        default.options = fix_options(options);
    }

    if let Some(inputs) = raw_value.get("inputs")
        && let Some(inputs) = inputs.as_table()
    {
        default.inputs = inputs
            .iter()
            .map(|(name, value)| (name.clone(), value.to_string()))
            .collect();
    }

    default
}

pub fn fix_application(broken: &Value) -> SaveApplication {
    let mut default = SaveApplication::default();

    if let Some(env) = broken.get("env").and_then(|v| v.as_array()) {
        let mut vec = Vec::new();

        for value in env {
            if let Some(pair) = value.as_array()
                && pair.len() == 2
                && let (Some(key), Some(val)) = (pair[0].as_str(), pair[1].as_str())
            {
                vec.push((key.to_string(), val.to_string()));
            }
        }

        default.env = vec;
    }

    if let Some(args) = broken.get("args").and_then(|v| v.as_array()) {
        let mut vec = Vec::new();
        for value in args {
            if let Some(str) = value.as_str() {
                vec.push(str.to_string());
            }
        }
        default.args = vec;
    }
    if let Some(executable) = broken.get("executable").and_then(|v| v.as_str()) {
        default.executable = executable.to_string();
    }

    default
}

pub fn fix_options(broken: &Value) -> SaveOptions {
    let mut default = SaveOptions::default();

    if let Some(width) = broken.get("width").and_then(|v| v.as_integer()) {
        default.width = width as i32;
    }
    if let Some(height) = broken.get("height").and_then(|v| v.as_integer()) {
        default.height = height as i32;
    }
    if let Some(hz) = broken.get("hz").and_then(|v| v.as_integer()) {
        default.hz = hz as i32;
    }
    if let Some(window) = broken.get("window").and_then(|v| v.as_str()) {
        default.window = Some(window.to_string());
    }
    if let Some(fullscreen) = broken.get("fullscreen").and_then(|v| v.as_bool()) {
        default.fullscreen = fullscreen;
    }
    if let Some(quiet) = broken.get("quiet").and_then(|v| v.as_bool()) {
        default.quiet = quiet;
    }

    default
}
