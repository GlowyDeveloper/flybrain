use crate::settings::{
    recovery::{fix_env_toml, fix_save_toml},
    structs::{EnvToml, SaveToml},
};
use std::path::PathBuf;
use tracing::{error, info, trace};

pub mod recovery;
pub mod structs;

pub fn dot_flybrain() -> PathBuf {
    let mut dir = std::env::current_exe().unwrap();
    dir.pop();
    dir.push(".flybrain");
    dir
}

pub fn initilize_tomls() {
    let root = dot_flybrain();

    if !root.exists() {
        std::fs::create_dir_all(&root).unwrap();
    }

    let saves = root.join("saves");
    if !saves.exists() {
        std::fs::create_dir_all(&saves).unwrap();
    }

    let env_path = root.join("env.toml");
    let env = if !env_path.exists() {
        let env = EnvToml::default();
        Some(toml::to_string_pretty(&env).unwrap())
    } else {
        let content = std::fs::read_to_string(&env_path).unwrap_or_default();

        match toml::from_str::<EnvToml>(&content) {
            Ok(_) => None,
            Err(err) => {
                println!("Failed to deserialize env.toml");
                info!("env.toml deserialization error: {}", err);
                let env = fix_env_toml(&content);
                Some(toml::to_string_pretty(&env).unwrap())
            }
        }
    };

    if let Some(data) = env {
        info!("Writing to {:#?}", env_path);
        trace!(data);
        std::fs::write(&env_path, data).unwrap();
    }

    let saves_children = std::fs::read_dir(saves).unwrap();
    for save in saves_children.flatten() {
        let file = save.path().join("save.toml");
        let content = std::fs::read_to_string(&file).unwrap_or_default();

        match toml::from_str::<SaveToml>(&content) {
            Ok(_) => {}
            Err(err) => {
                println!("Failed to deserialize {:#?}", file);
                info!("{:#?} deserialization error: {}", file, err);
                let saved = fix_save_toml(&content);
                let str = toml::to_string_pretty(&saved).unwrap();

                info!("Writing to {:#?}", file);
                trace!(str);
                std::fs::write(&file, str).unwrap();
            }
        }
    }
}

pub fn write_current(current: &SaveToml) {
    let path = dot_flybrain().join("current.toml");

    let str = toml::to_string_pretty(current).unwrap();

    info!("Writing to {:#?}", path);
    trace!(str);
    std::fs::write(&path, str).unwrap();
}

pub fn get_created(requested: String) -> SaveToml {
    let root = dot_flybrain();
    if root.join("current.toml").exists() {
        info!("current.toml exists, deleting");
        let _ = std::fs::remove_file(root.join("current.toml"));
    }

    let file = dot_flybrain()
        .join("saves")
        .join(requested.clone())
        .join("save.toml");

    if !file.exists() {
        eprintln!("File \"{}\" does not exist.", requested);
        std::process::exit(1);
    }
    if file.is_dir() {
        eprintln!("Expected file \"{}\", found folder.", requested);
    }

    let content = std::fs::read_to_string(file.clone()).unwrap_or_default();

    match toml::from_str::<SaveToml>(&content) {
        Ok(toml) => toml,
        Err(err) => {
            println!("Failed to deserialize {:#?}", file);
            info!("{:#?} deserialization error: {}", file, err);
            let saved = fix_save_toml(&content);
            let str = toml::to_string_pretty(&saved).unwrap();

            info!("Writing to {:#?}", file);
            trace!(str);
            std::fs::write(&file, str).unwrap();
            saved
        }
    }
}

pub fn update_or_create_save(requested: String, args: Vec<String>) -> Option<SaveToml> {
    let file = dot_flybrain()
        .join("saves")
        .join(requested.clone())
        .join("save.toml");

    std::fs::create_dir(dot_flybrain().join("saves").join(requested)).unwrap();

    let toml = if file.exists() {
        let content = std::fs::read_to_string(&file).unwrap_or_default();

        match toml::from_str::<SaveToml>(&content) {
            Ok(toml) => toml,
            Err(err) => {
                println!("Failed to deserialize {:#?}", file);
                info!("{:#?} deserialization error: {}", file, err);
                let saved = fix_save_toml(&content);
                let str = toml::to_string_pretty(&saved).unwrap();

                info!("Writing to {:#?}", file);
                trace!(str);
                std::fs::write(&file, str).unwrap();
                saved
            }
        }
    } else {
        SaveToml::default()
    };

    let updated = parse_args(args, toml);
    let updated = Some(updated?);

    let str = toml::to_string_pretty(&updated).unwrap();

    info!("Writing to {:#?}", file);
    trace!(str);
    std::fs::write(&file, str).unwrap();
    updated
}

pub fn launch_without_save(args: Vec<String>) -> Option<SaveToml> {
    parse_args(args, SaveToml::default())
}

pub fn parse_args(args: Vec<String>, mut toml: SaveToml) -> Option<SaveToml> {
    let (cmd, left) = {
        let cmd = args.iter().position(|arg| arg == "--")?;
        let (_, a) = args.split_at(cmd + 1);
        let (left, _) = args.split_at(cmd);
        (a.to_vec(), left)
    };

    let mut envs = Vec::new();
    let mut executable = None;
    let mut args = Vec::new();

    for item in cmd {
        if executable.is_none() {
            if let Some((key, value)) = item.split_once("=")
                && !key.is_empty()
            {
                envs.push((key.to_owned(), value.to_owned()));
                continue;
            }
            executable = Some(item);
        } else {
            args.push(item);
        }
    }

    if let Ok(host_runtime) = std::env::var("XDG_RUNTIME_DIR") {
        envs.push(("XDG_RUNTIME_DIR".to_string(), host_runtime.clone()));
        envs.push((
            "PRESSURE_VESSEL_FILESYSTEMS_RW".to_string(),
            format!("{}/flybrain", host_runtime),
        ));
    }

    envs.push(("WAYLAND_DISPLAY".to_string(), "flybrain".to_string()));

    if executable.is_none() {
        error!("Executable not specified.");
        return None;
    }

    let mut application = toml.application;
    application.args = args;
    application.env = envs;
    application.executable = executable.unwrap();

    let mut options = toml.options;

    if left.contains(&"-fullscreen".to_string()) {
        options.fullscreen = true;
    }

    if left.contains(&"-quiet".to_string()) {
        options.quiet = true;
    }

    if let Some(width) = left.iter().position(|arg| arg == "-width")
        && let Ok(width) = left[width].parse::<i32>()
    {
        options.width = width;
    }

    if let Some(height) = left.iter().position(|arg| arg == "-height")
        && let Ok(height) = left[height].parse::<i32>()
    {
        options.height = height;
    }

    if let Some(hz) = left.iter().position(|arg| arg == "-hz")
        && let Ok(hz) = left[hz].parse::<i32>()
    {
        options.hz = hz;
    }

    if let Some(window) = left.iter().position(|arg| arg == "-window") {
        let mut window = left[window].clone();
        if window.starts_with("\"") {
            window = window.strip_prefix("\"").unwrap().to_string();
        }
        if window.ends_with("\"") {
            window = window.strip_suffix("\"").unwrap().to_string();
        }
        options.window = Some(window);
    }

    toml.application = application;
    toml.options = options;

    Some(toml)
}

pub fn write_policy(name: Option<String>) {
    let current = dot_flybrain().join("current.npz");

    if let Some(name) = name {
        let file = dot_flybrain()
            .join("saves")
            .join(name.clone())
            .join("policy.npz");

        if file.exists() {
            std::fs::copy(file, current).unwrap();
        } else {
            std::fs::write(&file, "").unwrap();
            std::fs::write(&current, "").unwrap();
        }
    } else {
        std::fs::write(&current, "").unwrap();
    }
}
