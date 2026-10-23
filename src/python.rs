use crate::{ENDC, HEADER, HEIGHT, PYTHON, VERBOSITY, Verbosity, WIDTH};
use std::{process::{Command, Stdio}, io::Write};
use tracing::{debug, warn};

const FLYBRAIN_DOWNLOAD: &str = include_str!("../python/download.py");
const IPC: &str = include_str!("../python/main.py");

pub fn get_python() -> &'static str {
    let python = Command::new("python").arg("--version").output();

    if python.is_ok() {
        debug!("python: python");
        "python"
    } else {
        let python3 = Command::new("python3").arg("--version").output();
        if python3.is_ok() {
            debug!("python: python3");
            "python3"
        } else {
            debug!("python: py");
            "py"
        }
    }
}

pub fn install_pip_dependencies() -> std::io::Result<()> {
    let verbosity = *VERBOSITY.lock().unwrap();

    fn ensurepip() -> std::io::Result<()> {
        let verbosity = *VERBOSITY.lock().unwrap();

        warn!("ensure pip not found");

        let mut pip_install = Command::new(*PYTHON.lock().unwrap());
        pip_install.args(["-m", "ensurepip", "--upgrade"]);

        println!("{HEADER}Retrying flybrain{ENDC}");

        let mut pip_install = Command::new(*PYTHON.lock().unwrap());
        pip_install.args(["-m", "pip", "install", "flybrain"]);
        if verbosity == Verbosity::Verbose || verbosity == Verbosity::VeryVerbose {
            let _ = pip_install.status()?;
        } else {
            let _ = pip_install.output()?;
        }

        let mut pip_install = Command::new(*PYTHON.lock().unwrap());
        pip_install.args(["-m", "pip", "install", "numpy"]);
        if verbosity == Verbosity::Verbose || verbosity == Verbosity::VeryVerbose {
            let _ = pip_install.status()?;
        } else {
            let _ = pip_install.output()?;
        }

        Ok(())
    }

    println!("{HEADER}Installing flybrain{ENDC}");

    let mut pip_install = Command::new(*PYTHON.lock().unwrap());
    pip_install.args(["-m", "pip", "install", "flybrain"]);
    if verbosity == Verbosity::Verbose || verbosity == Verbosity::VeryVerbose {
        match pip_install.status() {
            Ok(_) => println!("{HEADER}Installed flybrain{ENDC}"),
            Err(_) => return ensurepip(),
        }
    } else {
        match pip_install.output() {
            Ok(_) => println!("{HEADER}Installed flybrain{ENDC}"),
            Err(_) => return ensurepip(),
        }
    }

    let mut pip_install = Command::new(*PYTHON.lock().unwrap());
    pip_install.args(["-m", "pip", "install", "numpy"]);
    if verbosity == Verbosity::Verbose || verbosity == Verbosity::VeryVerbose {
        match pip_install.status() {
            Ok(_) => println!("{HEADER}Installed flybrain{ENDC}"),
            Err(_) => return ensurepip(),
        }
    } else {
        match pip_install.output() {
            Ok(_) => println!("{HEADER}Installed flybrain{ENDC}"),
            Err(_) => return ensurepip(),
        }
    }

    Ok(())
}

pub fn install_synapses() -> std::io::Result<()> {
    println!("{HEADER}Installing fly brain synapses{ENDC}");

    let mut dir = std::env::current_exe().unwrap();
    dir.pop();
    dir.push(".flybrain/brain");

    let mut child = Command::new(*PYTHON.lock().unwrap())
        .arg("-")
        .arg(dir.to_str().unwrap())
        .stdin(Stdio::piped())
        .spawn()?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(FLYBRAIN_DOWNLOAD.as_bytes())?;
    }

    let status = child.wait()?;

    if !status.success() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            "Python script failed to execute successfully",
        ));
    }

    println!("{HEADER}Installed fly brain synapses{ENDC}");
    Ok(())
}

pub fn run_main_python() -> Result<(), Box<dyn std::error::Error>> {
    let lane_1 = *HEIGHT.lock().unwrap() * *WIDTH.lock().unwrap() * 4;
    let lane_2 = 1;

    std::thread::Builder::new()
        .name("python".to_string())
        .spawn(move || {
            let mut child = match Command::new(*PYTHON.lock().unwrap())
                .arg("-")
                .arg(lane_1.to_string())
                .arg(lane_2.to_string())
                .stdin(Stdio::piped())
                .spawn() 
            {
                Ok(c) => c,
                Err(_) => return,
            };

            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(IPC.as_bytes());
            }

            let _ = child.wait();
        })?;

    Ok(())
}
