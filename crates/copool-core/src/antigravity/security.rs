//! Antigravity's login item on macOS, reached only through
//! `/usr/bin/security`, as agy's go-keyring reaches it. Each write sets the
//! item's partition list to the writer's: written by `security`, it stays
//! `apple-tool:`, which agy, Copool and other apps reading it through the
//! tool may all use. Written by Copool itself, it would become Copool's
//! team, and agy's next access would ask for approval, as Copool's would
//! after agy's next write.
//!
//! The tool cannot be told to fail instead of asking for approval, so
//! background calls are given a time limit and stopped after it; the
//! answer then counts as unavailable. Calls take turns, so that waiting
//! approvals do not pile up.
use super::login::{ACCOUNT, ITEM, SERVICE};
use crate::{account_network::Response, vault::KeychainAccess};
use std::{sync::Arc, time::Duration};

type Result<T> = std::result::Result<T, String>;

const PROGRAM: &str = "/usr/bin/security";
/// `find-generic-password`'s status when there is no such item.
const NOT_FOUND: i32 = 44;
/// How long a background call may wait, as for an approval nobody answers.
const BACKGROUND_LIMIT: Duration = Duration::from_secs(60);
/// The longest line `security -i` reads.
const MAX_LINE: usize = 4095;

/// What a run of the tool reported.
pub(crate) struct Output {
    pub(crate) status: Option<i32>,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
}
/// Runs `/usr/bin/security`; tests use a fake.
pub(crate) trait Tool: Send + Sync {
    /// Runs it with these arguments and, when given, this input on its
    /// stdin. Dropping the answer stops the run.
    fn run(&self, args: Vec<String>, stdin: Option<Vec<u8>>) -> Response<'_, Output>;
}
/// The system's tool.
pub(crate) struct Security;
impl Tool for Security {
    fn run(&self, args: Vec<String>, stdin: Option<Vec<u8>>) -> Response<'_, Output> {
        Box::pin(async move {
            use std::process::Stdio;
            use tokio::io::AsyncWriteExt;
            let mut child = tokio::process::Command::new(PROGRAM)
                .args(&args)
                .kill_on_drop(true)
                .stdin(if stdin.is_some() {
                    Stdio::piped()
                } else {
                    Stdio::null()
                })
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| format!("{PROGRAM} could not run: {e}"))?;
            if let Some(input) = stdin {
                let mut pipe = child.stdin.take().ok_or("Keychain input unavailable")?;
                pipe.write_all(&input).await.map_err(|e| e.to_string())?;
            }
            let output = child.wait_with_output().await.map_err(|e| e.to_string())?;
            Ok(Output {
                status: output.status.code(),
                stdout: output.stdout,
                stderr: output.stderr,
            })
        })
    }
}

/// The item, through the tool.
pub(crate) struct Keychain {
    tool: Arc<dyn Tool>,
    limit: Duration,
    turn: tokio::sync::Mutex<()>,
}
impl Default for Keychain {
    fn default() -> Self {
        Self::new(Arc::new(Security), BACKGROUND_LIMIT)
    }
}
fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|a| (*a).to_owned()).collect()
}
/// The secret: go-keyring's `-w` prints it as text, or in hex when it is
/// not printable text (an unwrapped login with non-ASCII text, say; agy's
/// own wrapped logins are always ASCII).
fn password(stdout: &[u8]) -> Vec<u8> {
    let text = stdout.strip_suffix(b"\n").unwrap_or(stdout);
    let digits = text
        .strip_prefix(b"0x")
        .or_else(|| text.strip_prefix(b"0X"))
        .unwrap_or(text);
    // A login is JSON or go-keyring's wrapping, never hex digits alone.
    if !digits.is_empty()
        && digits.len().is_multiple_of(2)
        && digits.iter().all(u8::is_ascii_hexdigit)
    {
        let decoded = digits
            .chunks(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok())
            .collect::<Option<Vec<u8>>>();
        if let Some(bytes) = decoded {
            return bytes;
        }
    }
    text.to_vec()
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
/// The line `security -i` reads to store the bytes: go-keyring's own, with
/// `-w`, for text that needs no quoting (its `go-keyring-base64:` form);
/// otherwise the exact bytes in hex, with `-X`, which no quoting can alter.
fn write_command(bytes: &[u8]) -> Result<String> {
    let plain = !bytes.is_empty()
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || b"+/=:_.-".contains(b));
    let secret = if plain {
        format!("-w {}", String::from_utf8_lossy(bytes))
    } else {
        format!("-X {}", hex(bytes))
    };
    let command = format!("add-generic-password -U -s {SERVICE} -a {ACCOUNT} {secret}\n");
    // Never fall back to putting the login in the tool's arguments.
    if command.len() > MAX_LINE {
        return Err(format!("The login is too large to write to {ITEM}"));
    }
    Ok(command)
}
impl Keychain {
    pub(crate) fn new(tool: Arc<dyn Tool>, limit: Duration) -> Self {
        Self {
            tool,
            limit,
            turn: Default::default(),
        }
    }
    /// One run, in turn; a background one stopped after the time limit.
    async fn call(
        &self,
        args: Vec<String>,
        stdin: Option<Vec<u8>>,
        access: KeychainAccess,
    ) -> Result<Output> {
        let run = async {
            let _turn = self.turn.lock().await;
            self.tool.run(args, stdin).await
        };
        match access {
            // The user may be answering an approval.
            KeychainAccess::Interactive => run.await,
            KeychainAccess::Background => tokio::time::timeout(self.limit, run)
                .await
                .map_err(|_| format!("{ITEM} did not answer in time"))?,
        }
    }
    /// Whether the item exists; its attributes need no approval.
    async fn exists(&self, access: KeychainAccess) -> Result<bool> {
        let found = self
            .call(
                args(&["find-generic-password", "-s", SERVICE, "-a", ACCOUNT]),
                None,
                access,
            )
            .await?;
        match found.status {
            Some(0) => Ok(true),
            Some(NOT_FOUND) => Ok(false),
            status => Err(format!("{ITEM} access failed: {}", exit(status))),
        }
    }
}
fn exit(status: Option<i32>) -> String {
    match status {
        Some(code) => format!("security exited with status {code}"),
        None => "security was stopped".into(),
    }
}
impl super::login::LiveStore for Keychain {
    fn read(&self, access: KeychainAccess) -> Response<'_, Option<Vec<u8>>> {
        Box::pin(async move {
            let found = self
                .call(
                    args(&["find-generic-password", "-s", SERVICE, "-a", ACCOUNT, "-w"]),
                    None,
                    access,
                )
                .await?;
            match found.status {
                Some(0) => Ok(Some(password(&found.stdout))),
                Some(NOT_FOUND) => Ok(None),
                status => Err(format!("{ITEM} access failed: {}", exit(status))),
            }
        })
    }
    /// Updates the item, or creates it, which only an explicit action may.
    fn write(&self, bytes: Vec<u8>, access: KeychainAccess) -> Response<'_, ()> {
        Box::pin(async move {
            let command = write_command(&bytes)?;
            if access == KeychainAccess::Background && !self.exists(access).await? {
                return Err(format!("Cannot create {ITEM} in the background"));
            }
            // The login travels only over stdin, never in the arguments.
            let written = self
                .call(args(&["-i"]), Some(command.into_bytes()), access)
                .await?;
            // `security -i` can exit successfully after a failed command;
            // its errors go to stderr, with its `security>` prompt. Not echoed.
            let errors = String::from_utf8_lossy(&written.stderr).replace("security>", "");
            if written.status != Some(0) || !errors.trim().is_empty() {
                return Err(format!("Could not write {ITEM}"));
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::login::{self, Document, LiveStore};
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;

    /// `security` over one item in memory: it prints the secret as text, or
    /// in hex when it is not printable ASCII, as the tool does.
    #[derive(Default)]
    struct Fake {
        item: Mutex<Option<Vec<u8>>>,
        calls: Mutex<Vec<(Vec<String>, Option<String>)>>,
        /// Statuses to answer with instead, in turn.
        statuses: Mutex<Vec<i32>>,
        /// Never answers, as while an approval waits.
        hangs: bool,
        /// What `security -i` reports on stderr.
        stderr: &'static str,
    }
    impl Fake {
        fn calls(&self) -> Vec<(Vec<String>, Option<String>)> {
            self.calls.lock().unwrap().clone()
        }
    }
    fn output(status: i32, stdout: Vec<u8>, stderr: &str) -> Output {
        Output {
            status: Some(status),
            stdout,
            stderr: stderr.as_bytes().to_vec(),
        }
    }
    impl Tool for Fake {
        fn run(&self, args: Vec<String>, stdin: Option<Vec<u8>>) -> Response<'_, Output> {
            let input = stdin.map(|s| String::from_utf8(s).unwrap());
            self.calls
                .lock()
                .unwrap()
                .push((args.clone(), input.clone()));
            if self.hangs {
                return Box::pin(std::future::pending());
            }
            if let Some(status) = self.statuses.lock().unwrap().pop() {
                return Box::pin(async move { Ok(output(status, vec![], "")) });
            }
            let mut item = self.item.lock().unwrap();
            let answer = if args == ["-i"] {
                let line = input.unwrap();
                let line = line.strip_suffix('\n').unwrap();
                let prefix = format!("add-generic-password -U -s {SERVICE} -a {ACCOUNT} ");
                let secret = line.strip_prefix(&prefix).unwrap();
                *item = Some(if let Some(text) = secret.strip_prefix("-w ") {
                    text.as_bytes().to_vec()
                } else {
                    let digits = secret.strip_prefix("-X ").unwrap();
                    (0..digits.len())
                        .step_by(2)
                        .map(|i| u8::from_str_radix(&digits[i..i + 2], 16).unwrap())
                        .collect()
                });
                output(0, vec![], self.stderr)
            } else {
                match item.as_ref() {
                    None => output(NOT_FOUND, vec![], ""),
                    Some(_) if !args.contains(&"-w".to_owned()) => output(0, vec![], ""),
                    Some(bytes) => {
                        let printable = bytes.iter().all(|b| (0x20..0x7f).contains(b));
                        let mut shown = if printable {
                            bytes.clone()
                        } else {
                            hex(bytes).into_bytes()
                        };
                        shown.push(b'\n');
                        output(0, shown, "")
                    }
                }
            };
            Box::pin(async move { Ok(answer) })
        }
    }
    fn keychain(fake: &Arc<Fake>) -> Keychain {
        Keychain::new(fake.clone(), Duration::from_millis(50))
    }
    const READ: [&str; 6] = [
        "find-generic-password",
        "-s",
        "gemini",
        "-a",
        "antigravity",
        "-w",
    ];
    const FIND: [&str; 5] = ["find-generic-password", "-s", "gemini", "-a", "antigravity"];

    #[tokio::test]
    async fn the_item_is_read_with_the_tool_and_may_be_absent() {
        let fake = Arc::new(Fake::default());
        let store = keychain(&fake);
        assert_eq!(store.read(KeychainAccess::Background).await, Ok(None));
        *fake.item.lock().unwrap() = Some(b"go-keyring-base64:eyJhIjoxfQ==".to_vec());
        assert_eq!(
            store.read(KeychainAccess::Interactive).await,
            Ok(Some(b"go-keyring-base64:eyJhIjoxfQ==".to_vec()))
        );
        assert_eq!(fake.calls(), [(args(&READ), None), (args(&READ), None)]);
        // Refused or denied: an error, not an absent login.
        fake.statuses.lock().unwrap().push(51);
        assert!(store.read(KeychainAccess::Background).await.is_err());
    }

    #[test]
    fn hex_output_is_the_secret_in_hex() {
        let json = r#"{"token":{"refresh_token":"r"},"id_token_email":"zoë@example.com"}"#;
        assert_eq!(
            password(format!("{}\n", hex(json.as_bytes())).as_bytes()),
            json.as_bytes()
        );
        assert_eq!(
            password(format!("0x{}\n", hex(b"\x01a")).as_bytes()),
            b"\x01a"
        );
        // Text stays text.
        assert_eq!(
            password(b"go-keyring-base64:YQ==\n"),
            b"go-keyring-base64:YQ=="
        );
        assert_eq!(password(b"{\"a\":1}"), b"{\"a\":1}");
    }

    #[tokio::test]
    async fn writes_send_the_login_only_on_stdin_in_its_form() {
        let fake = Arc::new(Fake::default());
        *fake.item.lock().unwrap() = Some(b"old".to_vec());
        let store = keychain(&fake);
        let value = json!({"token": {"access_token": "a", "refresh_token": "r", "expiry": "2026-07-12T10:00:00Z"}, "id_token_email": "zoë@example.com"});
        for wrapped in [true, false] {
            fake.calls.lock().unwrap().clear();
            let document = Document {
                value: value.clone(),
                wrapped,
            };
            let bytes = login::encode(&document);
            store
                .write(bytes.clone(), KeychainAccess::Background)
                .await
                .unwrap();
            let calls = fake.calls();
            // Background: the item's existence first, which needs no approval.
            assert_eq!(calls[0], (args(&FIND), None));
            let (argv, stdin) = &calls[1];
            assert_eq!(argv, &args(&["-i"]));
            let stdin = stdin.as_deref().unwrap();
            let expected = if wrapped {
                format!(
                    "add-generic-password -U -s gemini -a antigravity -w {}\n",
                    String::from_utf8(bytes.clone()).unwrap()
                )
            } else {
                format!(
                    "add-generic-password -U -s gemini -a antigravity -X {}\n",
                    hex(&bytes)
                )
            };
            assert_eq!(stdin, expected);
            // No argument of any call carries the login.
            assert!(calls
                .iter()
                .flat_map(|(argv, _)| argv)
                .all(|a| !a.contains("go-keyring") && !a.contains("refresh") && a.len() < 30));
            // Read back as agy and Copool read it, in the same form.
            assert_eq!(fake.item.lock().unwrap().as_deref(), Some(bytes.as_slice()));
            let read = store
                .read(KeychainAccess::Background)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(login::decode(&read), Some(document));
        }
    }

    #[tokio::test]
    async fn only_an_explicit_action_creates_the_item() {
        let fake = Arc::new(Fake::default());
        let store = keychain(&fake);
        assert!(store
            .write(
                b"go-keyring-base64:YQ==".to_vec(),
                KeychainAccess::Background
            )
            .await
            .is_err());
        assert_eq!(fake.calls(), [(args(&FIND), None)]);
        store
            .write(
                b"go-keyring-base64:YQ==".to_vec(),
                KeychainAccess::Interactive,
            )
            .await
            .unwrap();
        assert_eq!(fake.calls().len(), 2);
        assert_eq!(
            fake.item.lock().unwrap().as_deref(),
            Some(b"go-keyring-base64:YQ==".as_slice())
        );
    }

    #[tokio::test]
    async fn a_failed_command_or_an_oversized_login_is_an_error() {
        let fake = Arc::new(Fake {
            item: Mutex::new(Some(b"old".to_vec())),
            stderr: "security> security: User interaction is not allowed.\n",
            ..Default::default()
        });
        let store = keychain(&fake);
        let error = store
            .write(
                b"go-keyring-base64:YQ==".to_vec(),
                KeychainAccess::Interactive,
            )
            .await
            .unwrap_err();
        assert!(!error.contains("YQ=="));
        let calls = fake.calls().len();
        assert!(store
            .write(vec![b'a'; 4096], KeychainAccess::Interactive)
            .await
            .is_err());
        assert_eq!(fake.calls().len(), calls);
    }

    #[tokio::test]
    async fn a_background_call_waiting_for_approval_is_given_up() {
        let fake = Arc::new(Fake {
            hangs: true,
            ..Default::default()
        });
        let store = keychain(&fake);
        assert!(store.read(KeychainAccess::Background).await.is_err());
        assert!(store
            .write(
                b"go-keyring-base64:YQ==".to_vec(),
                KeychainAccess::Background
            )
            .await
            .is_err());
        // An explicit action waits for the user's answer.
        let waiting = tokio::time::timeout(
            Duration::from_millis(200),
            store.read(KeychainAccess::Interactive),
        )
        .await;
        assert!(waiting.is_err());
    }
}
