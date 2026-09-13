//! Windows service wrapper for `hashgram-node`.
//!
//! The Service Control Manager starts this binary; it launches
//! `hashgram-node.exe run --home <home> --config <config>` as a child,
//! appends the node's output to `<home>\node.log`, restarts the child if it
//! dies (with back-off), and stops it cleanly when the service is stopped.
//! Arguments are read from the service's own command line as registered by
//! Hashgram for Windows:
//!
//! ```text
//! hashgram-node-service.exe --node <path\hashgram-node.exe> --home <dir> --config <file>
//! ```
//!
//! Run without `--service` it does the same in the foreground, which is
//! what the per-user Scheduled Task uses on machines without administrator
//! rights. `--config` defaults to `<home>\node.toml`.
//!
//! Built as a windowless program on Windows release builds so a logon task
//! never flashes a console at the user; the node child is started with
//! `CREATE_NO_WINDOW` for the same reason and its output goes to `node.log`.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

struct Args {
    node: PathBuf,
    home: PathBuf,
    config: PathBuf,
    service: bool,
}

fn parse(argv: &[String]) -> anyhow::Result<Args> {
    let mut node = None;
    let mut home = None;
    let mut config = None;
    let mut service = false;
    let mut i = 0;
    while i < argv.len() {
        match argv.get(i).map(String::as_str) {
            Some("--node") => {
                node = argv.get(i + 1).map(PathBuf::from);
                i += 1;
            }
            Some("--home") => {
                home = argv.get(i + 1).map(PathBuf::from);
                i += 1;
            }
            Some("--config") => {
                config = argv.get(i + 1).map(PathBuf::from);
                i += 1;
            }
            Some("--service") => service = true,
            _ => {}
        }
        i += 1;
    }
    let home: PathBuf = home.ok_or_else(|| anyhow::anyhow!("--home <dir> is required"))?;
    let config = config.unwrap_or_else(|| home.join("node.toml"));
    Ok(Args {
        node: node.ok_or_else(|| anyhow::anyhow!("--node <hashgram-node.exe> is required"))?,
        home,
        config,
        service,
    })
}

fn spawn(args: &Args) -> anyhow::Result<Child> {
    std::fs::create_dir_all(&args.home)?;
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(args.home.join("node.log"))?;
    let err = log.try_clone()?;
    let mut cmd = Command::new(&args.node);
    cmd.arg("run")
        .arg("--home")
        .arg(&args.home)
        .arg("--config")
        .arg(&args.config)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(err));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: the node is a console program; without this a
        // windowless parent would give it a console of its own on screen.
        cmd.creation_flags(0x0800_0000);
    }
    Ok(cmd.spawn()?)
}

/// Supervises the child until `stop` is set. Restarts with back-off.
fn supervise(args: &Args, stop: &AtomicBool) -> anyhow::Result<()> {
    let mut backoff = Duration::from_secs(2);
    while !stop.load(Ordering::SeqCst) {
        let mut child = spawn(args)?;
        loop {
            if stop.load(Ordering::SeqCst) {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(());
            }
            match child.try_wait()? {
                Some(status) => {
                    let _ = append(
                        &args.home,
                        &format!("hashgram-node exited with {status}; restarting in {backoff:?}"),
                    );
                    std::thread::sleep(backoff);
                    backoff = (backoff * 2).min(Duration::from_secs(120));
                    break;
                }
                None => {
                    std::thread::sleep(Duration::from_millis(500));
                    // Healthy for a while: reset the back-off.
                    backoff = Duration::from_secs(2);
                }
            }
        }
    }
    Ok(())
}

fn append(home: &std::path::Path, line: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(home.join("node.log"))?;
    writeln!(f, "[service] {line}")
}

#[cfg(windows)]
mod svc {
    use super::*;
    use windows_service::service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
        ServiceType,
    };
    use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
    use windows_service::{define_windows_service, service_dispatcher};

    pub(crate) const NAME: &str = "HashgramNode";

    define_windows_service!(ffi_service_main, service_main);

    fn service_main(_argv: Vec<std::ffi::OsString>) {
        // The SCM passes no arguments of ours; read the registered command line.
        let argv: Vec<String> = std::env::args().skip(1).collect();
        if let Err(e) = run_service(&argv) {
            eprintln!("hashgram-node-service: {e:#}");
        }
    }

    fn run_service(argv: &[String]) -> anyhow::Result<()> {
        let args = parse(argv)?;
        let stop = Arc::new(AtomicBool::new(false));
        let stop_h = stop.clone();
        let handler = move |control: ServiceControl| match control {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                stop_h.store(true, Ordering::SeqCst);
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        };
        let status_handle = service_control_handler::register(NAME, handler)?;
        let running = |state: ServiceState| ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: state,
            controls_accepted: ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::from_secs(10),
            process_id: None,
        };
        status_handle.set_service_status(running(ServiceState::Running))?;
        let result = supervise(&args, &stop);
        status_handle.set_service_status(running(ServiceState::Stopped))?;
        result
    }

    pub(crate) fn dispatch() -> anyhow::Result<()> {
        service_dispatcher::start(NAME, ffi_service_main)?;
        Ok(())
    }
}

fn main() -> anyhow::Result<()> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let args = parse(&argv)?;
    if args.service {
        #[cfg(windows)]
        {
            return svc::dispatch();
        }
        #[cfg(not(windows))]
        {
            anyhow::bail!("--service is Windows-only");
        }
    }
    // Foreground (Scheduled Task) mode: supervise until killed.
    let stop = AtomicBool::new(false);
    supervise(&args, &stop)
}
