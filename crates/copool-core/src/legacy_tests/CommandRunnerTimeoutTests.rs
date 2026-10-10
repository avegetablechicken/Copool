use crate::process::{run, CommandError};
#[tokio::test]
async fn testRunTimeoutThrowsAndReturnsPromptly() {
    let start = std::time::Instant::now();
    #[cfg(unix)]
    let (program, args, description) = ("/bin/sh", vec!["-c", "sleep 5"], "sleep 5");
    #[cfg(windows)]
    let (program, args, description) = (
        "powershell",
        vec![
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Start-Sleep -Seconds 5",
        ],
        "Start-Sleep -Seconds 5",
    );
    let error = run(program, &args, std::time::Duration::from_secs(1))
        .await
        .unwrap_err();
    assert!(matches!(error, CommandError::Timeout(_)));
    assert!(!error.to_string().is_empty());
    assert!(error.to_string().contains(description));
    assert!(start.elapsed().as_secs_f64() < 4.);
}
