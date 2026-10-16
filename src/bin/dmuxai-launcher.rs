#[cfg(not(windows))]
fn main() {
    eprintln!("dmuxai-launcher is only supported on Windows.");
    std::process::exit(1);
}

#[cfg(windows)]
fn main() {
    use dmuxai::launcher::{
        CheckResult, DistroChoice, LauncherConfig, MntGateResult, Probe, RealProbe,
        VersionGateResult, build_launch_command, choose_distro, mnt_gate, run_checks, version_gate,
        windows_to_wsl_path,
    };

    let probe = RealProbe;
    let config_path = LauncherConfig::default_path();
    let mut config = config_path
        .as_deref()
        .map(LauncherConfig::load)
        .unwrap_or_default();

    let mut allow_mnt = config.allow_mnt;
    let mut workspace_arg: Option<String> = None;
    let mut distro_arg: Option<String> = None;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--allow-mnt" => {
                allow_mnt = true;
            }
            "-d" | "--distro" => {
                if i + 1 < args.len() {
                    distro_arg = Some(args[i + 1].clone());
                    i += 1;
                }
            }
            arg if !arg.starts_with('-') && workspace_arg.is_none() => {
                workspace_arg = Some(arg.to_string());
            }
            _ => {}
        }
        i += 1;
    }

    if !probe.wsl_installed() {
        eprintln!("Blocked: WSL is not installed or not running");
        eprintln!("Fix-it: wsl --install");
        std::process::exit(1);
    }

    let distros = match probe.list_distros() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error listing WSL distros: {e}");
            std::process::exit(1);
        }
    };

    let selected_distro = if let Some(d) = distro_arg {
        d
    } else {
        match choose_distro(config.distro.as_deref(), &distros) {
            DistroChoice::Auto(d) | DistroChoice::Remembered(d) => d,
            DistroChoice::NeedsPicker(available) => {
                println!("Select WSL distribution:");
                for (idx, d) in available.iter().enumerate() {
                    println!("  [{}] {}", idx + 1, d);
                }
                print!("Choice [1-{}]: ", available.len());
                use std::io::Write;
                let _ = std::io::stdout().flush();
                let mut input = String::new();
                if std::io::stdin().read_line(&mut input).is_ok() {
                    if let Ok(choice) = input.trim().parse::<usize>() {
                        if (1..=available.len()).contains(&choice) {
                            available[choice - 1].clone()
                        } else {
                            eprintln!("Invalid choice");
                            std::process::exit(1);
                        }
                    } else {
                        eprintln!("Invalid input");
                        std::process::exit(1);
                    }
                } else {
                    eprintln!("Failed to read input");
                    std::process::exit(1);
                }
            }
            DistroChoice::Blocked { reason, fixit } => {
                eprintln!("Blocked: {reason}");
                eprintln!("Fix-it: {fixit}");
                std::process::exit(1);
            }
        }
    };

    match run_checks(&probe, Some(&selected_distro)) {
        CheckResult::Blocked { reason, fixit } => {
            eprintln!("Blocked: {reason}");
            eprintln!("Fix-it: {fixit}");
            std::process::exit(1);
        }
        CheckResult::Ok { warnings } => {
            for warn in warnings {
                println!(
                    "Notice: Agent CLI '{}' not found inside distro '{}'. Install with: {}",
                    warn.name, selected_distro, warn.fixit
                );
            }
        }
    }

    match version_gate(&probe, &selected_distro) {
        VersionGateResult::Ok => {}
        VersionGateResult::Stale {
            current,
            required,
            fixit,
        } => {
            eprintln!(
                "Error: Installed dmuxai version inside distro '{selected_distro}' ({current}) is older than required minimum ({required})."
            );
            eprintln!("Fix-it: {fixit}");
            std::process::exit(1);
        }
        VersionGateResult::Unverified { warning } => {
            println!("Notice: {warning}");
        }
    }

    let win_workspace = workspace_arg
        .or(config.last_workspace.clone())
        .unwrap_or_else(|| {
            std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| ".".to_string())
        });

    let wsl_workspace = match windows_to_wsl_path(&probe, &selected_distro, &win_workspace) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error converting workspace path: {e}");
            std::process::exit(1);
        }
    };

    match mnt_gate(&wsl_workspace, allow_mnt) {
        MntGateResult::Allowed => {}
        MntGateResult::WarnAndPrompt { warning } => {
            println!("Warning: {warning}");
            print!("Proceed anyway? [y/N]: ");
            use std::io::Write;
            let _ = std::io::stdout().flush();
            let mut answer = String::new();
            if std::io::stdin().read_line(&mut answer).is_err()
                || !answer.trim().eq_ignore_ascii_case("y")
            {
                eprintln!("Launch cancelled.");
                std::process::exit(1);
            }
            allow_mnt = true;
        }
    }

    config.distro = Some(selected_distro.clone());
    config.last_workspace = Some(win_workspace);
    config.allow_mnt = allow_mnt;
    if let Some(ref path) = config_path {
        let _ = config.save(path);
    }

    let launch_cmd = match build_launch_command(&probe, &selected_distro, &wsl_workspace) {
        Ok(cmd) => cmd,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };
    if let Some(notice) = launch_cmd.notice {
        println!("Notice: {notice}");
    }

    let mut cmd = std::process::Command::new(&launch_cmd.argv[0]);
    cmd.args(&launch_cmd.argv[1..]);

    match cmd.status() {
        Ok(status) => {
            if let Some(code) = status.code() {
                std::process::exit(code);
            }
        }
        Err(e) => {
            eprintln!("Failed to launch terminal: {e}");
            std::process::exit(1);
        }
    }
}
