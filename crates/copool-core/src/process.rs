use std::{fmt, process::Output, time::Duration};
#[derive(Debug)]
pub enum CommandError {
    Io(std::io::Error),
    Timeout(String),
}
impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => e.fmt(f),
            Self::Timeout(command) => write!(f, "Command timed out: {command}"),
        }
    }
}
impl std::error::Error for CommandError {}
pub async fn run(program: &str, args: &[&str], timeout: Duration) -> Result<Output, CommandError> {
    let child = tokio::process::Command::new(program)
        .args(args)
        .kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(CommandError::Io)?;
    tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| CommandError::Timeout(format!("{program} {}", args.join(" "))))?
        .map_err(CommandError::Io)
}
