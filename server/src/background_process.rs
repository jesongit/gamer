//! Background tool processes must not create console windows in the desktop app.
//! This only controls window creation; arguments, IO, environment and lifetime
//! remain the caller's responsibility. Other platforms keep their normal behavior.
use std::ffi::OsStr;

pub fn command(program: impl AsRef<OsStr>) -> std::process::Command {
    let mut command = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    // Keep construction uniform across platforms, including the mutable Windows setup.
    #[cfg(not(windows))]
    let _ = &mut command;
    command
}

pub fn tokio_command(program: impl AsRef<OsStr>) -> tokio::process::Command {
    command(program).into()
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    const CHILD_ENV: &str = "GAMER_BACKGROUND_PROCESS_PROBE";
    const CHILD_TEST: &str = "background_process::tests::console_probe_child";

    #[test]
    fn console_probe_child() {
        if std::env::var(CHILD_ENV).ok().as_deref() != Some("probe") {
            return;
        }
        let console = unsafe { windows_sys::Win32::System::Console::GetConsoleWindow() };
        assert!(
            console.is_null(),
            "background child received a console window"
        );
        println!("background-stdout-ok");
        eprintln!("background-stderr-ok");
    }

    fn check_output(output: std::process::Output) {
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("background-stdout-ok"));
        assert!(String::from_utf8_lossy(&output.stderr).contains("background-stderr-ok"));
    }

    #[test]
    fn synchronous_child_has_no_console_and_keeps_piped_output() {
        let output = command(std::env::current_exe().unwrap())
            .args(["--exact", CHILD_TEST, "--nocapture"])
            .env(CHILD_ENV, "probe")
            .output()
            .unwrap();
        check_output(output);
    }

    #[tokio::test]
    async fn asynchronous_child_has_no_console_and_keeps_piped_output() {
        let mut command = tokio_command(std::env::current_exe().unwrap());
        command
            .args(["--exact", CHILD_TEST, "--nocapture"])
            .env(CHILD_ENV, "probe")
            .kill_on_drop(true);
        let output = tokio::time::timeout(std::time::Duration::from_secs(15), command.output())
            .await
            .unwrap()
            .unwrap();
        check_output(output);
    }
}
