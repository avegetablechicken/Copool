//! The Env bridge: Copool's block in a tool's `.env`, and the line in the
//! user's shell startup file that loads it, through which a switch to a
//! Sub2api provider gives the tool its key and gateway.
//!
//! A pool names its bridge ([`Bridge`]) and the lines its block holds; the
//! bridge writes them, has the startup file load the `.env` when a new shell
//! does not export what the pool expects ([`Expect`]), and takes it all back
//! when even then it does not.
use std::path::{Path, PathBuf};
use std::sync::Arc;

type Result<T> = std::result::Result<T, String>;

/// The variables a shell exports, by name: with `now`, as one opened now
/// does (loading the `.env` through the startup file); otherwise the user's
/// own exports, as the pool asks for them.
pub(crate) type Environment = Arc<dyn Fn(&[&str], bool) -> Vec<Option<String>> + Send + Sync>;

/// A pool's bridge. Its texts stay byte for byte as they were first
/// written: a `.env` or startup file from an earlier switch is recognized
/// by them, and a startup file that already ends with the loader is not
/// appended to again.
pub(crate) struct Bridge {
    /// The line opening Copool's block in the `.env`.
    pub block_start: &'static str,
    /// The comment line above the loader in the shell's startup file.
    pub rc_comment: &'static str,
    /// The variable naming a smoke test's own startup file.
    pub rc_override: &'static str,
}

/// The line closing Copool's block, the same for every bridge.
const BLOCK_END: &str = "# <<< Copool <<<";

/// What a new shell must export for the bridge to count as loaded.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Expect<'a> {
    /// The first variable is exported, with the second one's value.
    Same(&'a str, &'a str),
    /// The variable is exported as this address, ignoring surrounding space
    /// and trailing slashes.
    Address(&'a str, &'a str),
}

/// What the bridge changed, to take back when a later step fails.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Changes {
    env: Option<(PathBuf, Option<String>)>,
    rc: Option<(PathBuf, Appended)>,
}
impl Changes {
    /// Puts the `.env` back as it was and takes back the loader line, while
    /// the startup file still ends with it as it was left.
    pub fn undo(&self) {
        if let Some((rc, appended)) = &self.rc {
            unload_env_in(rc, appended);
        }
        if let Some((env, previous)) = &self.env {
            let _ = put_text(env, previous.as_deref());
        }
    }
}

impl Bridge {
    /// Whether the text holds Copool's block.
    pub fn has_block(&self, text: Option<&str>) -> bool {
        text.is_some_and(|text| text.lines().any(|line| line.trim_end() == self.block_start))
    }
    /// Whether Copool's block in the text holds this line.
    pub fn block_has_line(&self, text: Option<&str>, expected: &str) -> bool {
        let Some(text) = text else {
            return false;
        };
        let mut inside = false;
        for line in text.lines() {
            let trimmed = line.trim_end();
            if trimmed == self.block_start {
                inside = true;
            } else if trimmed == BLOCK_END {
                inside = false;
            } else if inside && trimmed == expected {
                return true;
            }
        }
        false
    }
    /// The text without Copool's block and the blank line `with_block` put
    /// before it, its other lines as they were.
    fn without_block(&self, text: &str) -> String {
        let mut inside = false;
        let mut kept: Vec<&str> = Vec::new();
        for line in text.split_inclusive('\n') {
            let trimmed = line.trim_end();
            if trimmed == self.block_start {
                inside = true;
                if kept.last().is_some_and(|l| l.trim().is_empty()) {
                    kept.pop();
                }
            }
            if !inside {
                kept.push(line);
            }
            if trimmed == BLOCK_END {
                inside = false;
            }
        }
        kept.concat()
    }
    /// The text with Copool's block of these lines at its end, set off from
    /// the user's lines by a blank one, in the file's newlines.
    pub(crate) fn with_block(&self, text: Option<&str>, lines: &[String]) -> String {
        let text = self.without_block(text.unwrap_or(""));
        let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
        let separator = if text.is_empty() {
            String::new()
        } else if text.ends_with('\n') {
            newline.to_owned()
        } else {
            newline.repeat(2)
        };
        let block: Vec<&str> = std::iter::once(self.block_start)
            .chain(lines.iter().map(String::as_str))
            .chain(std::iter::once(BLOCK_END))
            .collect();
        format!("{text}{separator}{}{newline}", block.join(newline))
    }
    /// The lines a shell's startup file loads the `.env` with, exporting
    /// what it sets; zsh and bash both read them. Not when Copool asks what
    /// the user exports (`OWN_EXPORTS_ONLY`): a key assigned there from
    /// another variable is not a key the user exported under its name.
    fn rc_lines(&self, env: &Path) -> Result<String> {
        // Absolute: a relative one would load a `.env` wherever a shell starts.
        let env = env
            .to_str()
            .filter(|p| env.is_absolute() && !p.contains('\'') && !p.contains(char::is_control))
            .ok_or("Invalid .env path")?;
        let own = crate::proxy_integration::OWN_EXPORTS_ONLY;
        Ok(format!(
            "{}\n[ -z \"${{{own}:-}}\" ] && [ -f '{env}' ] && {{ set -a; . '{env}'; set +a; }}\n",
            self.rc_comment
        ))
    }
    /// Has the startup file load `.env` last, after anything else it
    /// exports, unless it already ends so; none when nothing changed.
    fn load_env_in(&self, rc: &Path, env: &Path) -> Result<Option<Appended>> {
        use std::io::Write;
        let lines = self.rc_lines(env)?;
        let text = read_text(rc)?;
        if text.as_deref().is_some_and(|t| t.ends_with(&lines)) {
            return Ok(None);
        }
        let before = text.as_ref().map(|t| t.len() as u64);
        let separator = match text.as_deref() {
            None | Some("") => "",
            Some(t) if t.ends_with('\n') => "\n",
            Some(_) => "\n\n",
        };
        let text = format!("{separator}{lines}");
        // Appended in place: the file keeps its mode and any link to it.
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(rc)
            .and_then(|mut file| file.write_all(text.as_bytes()))
            .map_err(|e| e.to_string())?;
        Ok(Some(Appended { before, text }))
    }

    /// Writes Copool's block of these lines in the `.env` and checks that a
    /// shell opened now exports what is expected; when it does not, has the
    /// startup file (`rc`) load the `.env` last and checks again. Takes all
    /// back and fails with `not_loaded` when even then it does not.
    pub async fn put(
        &self,
        env: &Path,
        lines: &[String],
        rc: Option<PathBuf>,
        environment: Environment,
        expect: &[Expect<'_>],
        not_loaded: &str,
    ) -> Result<Changes> {
        let before = read_text(env)?;
        put_text(env, Some(&self.with_block(before.as_deref(), lines)))?;
        let mut changes = Changes {
            env: Some((env.to_owned(), before)),
            rc: None,
        };
        let mut loaded = exports(environment.clone(), expect).await;
        if let (Ok(false), Some(rc)) = (&loaded, rc) {
            loaded = match self.load_env_in(&rc, env) {
                Ok(Some(appended)) => {
                    changes.rc = Some((rc, appended));
                    exports(environment, expect).await
                }
                // It loads the file last already: something else differs.
                Ok(None) => Ok(false),
                Err(error) => Err(error),
            };
        }
        match loaded {
            Ok(true) => Ok(changes),
            other => {
                changes.undo();
                Err(other.err().unwrap_or_else(|| not_loaded.to_owned()))
            }
        }
    }
    /// Removes Copool's block from the `.env`, and the file when nothing
    /// else is left in it; nothing changes without a block.
    pub fn remove(&self, env: &Path) -> Result<Changes> {
        let before = read_text(env)?;
        if !self.has_block(before.as_deref()) {
            return Ok(Changes::default());
        }
        let left = self.without_block(before.as_deref().unwrap_or(""));
        put_text(env, (!left.trim().is_empty()).then_some(&*left))?;
        Ok(Changes {
            env: Some((env.to_owned(), before)),
            rc: None,
        })
    }
    /// The interactive startup file of the user's `$SHELL`, zsh's or bash's;
    /// none for another shell, or where Copool asks no shell. A smoke test
    /// names its own in the bridge's `rc_override`.
    #[cfg(not(test))]
    pub fn shell_rc(&self) -> Option<PathBuf> {
        if let Some(rc) = std::env::var_os(self.rc_override).filter(|v| !v.is_empty()) {
            return Some(rc.into());
        }
        if !cfg!(unix) {
            return None;
        }
        let shell = PathBuf::from(std::env::var_os("SHELL")?);
        let home = dirs::home_dir()?;
        match shell.file_name()?.to_str()? {
            "zsh" => Some(
                std::env::var_os("ZDOTDIR")
                    .filter(|v| !v.is_empty())
                    .map_or(home, PathBuf::from)
                    .join(".zshrc"),
            ),
            "bash" => Some(home.join(".bashrc")),
            _ => None,
        }
    }
}

/// Variables in a shell's environment, asked off the runtime, for the shell
/// may take seconds; none for each it does not export.
pub(crate) async fn exported(
    environment: Environment,
    names: &[&str],
    now: bool,
) -> Result<Vec<Option<String>>> {
    let names: Vec<String> = names.iter().map(|&name| name.to_owned()).collect();
    tokio::task::spawn_blocking(move || {
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut values = environment(&names, now);
        values.resize(names.len(), None);
        values
    })
    .await
    .map_err(|e| e.to_string())
}
/// Whether a shell opened now exports what is expected.
async fn exports(environment: Environment, expect: &[Expect<'_>]) -> Result<bool> {
    let mut names = Vec::new();
    for e in expect {
        match *e {
            Expect::Same(name, other) => names.extend([name, other]),
            Expect::Address(name, _) => names.push(name),
        }
    }
    let values = exported(environment, &names, true).await?;
    let mut values = values.iter();
    let mut next = || values.next().cloned().flatten();
    let address = |url: &str| url.trim().trim_end_matches('/').to_owned();
    let mut all = true;
    for e in expect {
        all &= match *e {
            Expect::Same(..) => {
                let (value, other) = (next(), next());
                value.is_some() && value == other
            }
            Expect::Address(_, url) => next().map(|v| address(&v)) == Some(address(url)),
        };
    }
    Ok(all)
}

/// What `load_env_in` appended to a startup file, for `unload_env_in`:
/// the file's length before, none when it was absent, and the text added.
#[derive(Debug, PartialEq)]
struct Appended {
    before: Option<u64>,
    text: String,
}
/// Takes back what `load_env_in` appended, while the file still ends with
/// it as it was left: an edit made since, by the user or another program,
/// stays whole.
fn unload_env_in(rc: &Path, appended: &Appended) {
    let Ok(Some(now)) = read_text(rc) else {
        return;
    };
    let before = appended.before.unwrap_or(0);
    if now.len() as u64 != before + appended.text.len() as u64 || !now.ends_with(&appended.text) {
        return;
    }
    let _ = match appended.before {
        None => std::fs::remove_file(rc),
        Some(len) => std::fs::OpenOptions::new()
            .write(true)
            .open(rc)
            .and_then(|file| file.set_len(len)),
    };
}

/// A file's text; none when there is none.
fn read_text(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}
/// Writes a file in place of a symlink's target, or removes it for none.
fn put_text(path: &Path, text: Option<&str>) -> Result<()> {
    match text {
        Some(text) => crate::claude::write_text(path, text),
        None => match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
            _ => Ok(()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    const BRIDGE: Bridge = Bridge {
        block_start: "# >>> Copool: a tool's Sub2api provider (rewritten on each switch) >>>",
        rc_comment: "# Copool: a tool's Sub2api key, from its .env.",
        rc_override: "COPOOL_TEST_SHELL_RC",
    };
    const START: &str = BRIDGE.block_start;

    fn lines(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|&l| l.to_owned()).collect()
    }
    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    fn read(path: &Path) -> Option<String> {
        read_text(path).unwrap()
    }
    /// A shell exporting `exports`, and the `.env` too once `loads` says the
    /// startup file loads it.
    fn shell(exports: &[(&str, &str)], env: &Path, loads: Arc<Mutex<bool>>) -> Environment {
        let exports: HashMap<String, String> = exports
            .iter()
            .map(|&(k, v)| (k.to_owned(), v.to_owned()))
            .collect();
        let env = env.to_owned();
        Arc::new(move |names: &[&str], _now: bool| {
            let mut vars = exports.clone();
            if *loads.lock().unwrap() {
                for line in read_text(&env).unwrap().unwrap_or_default().lines() {
                    if let Some((k, v)) = line.split_once('=') {
                        let v = v.trim_matches(['"', '\'']);
                        let v = match v.strip_prefix("${").and_then(|v| v.strip_suffix('}')) {
                            Some(other) => exports.get(other).cloned().unwrap_or_default(),
                            None => v.to_owned(),
                        };
                        vars.insert(k.to_owned(), v);
                    }
                }
            }
            names.iter().map(|n| vars.get(*n).cloned()).collect()
        })
    }

    #[test]
    fn copools_block_ends_env_and_leaves_the_users_lines() {
        let own = "# mine\nKEY=k\n";
        let applied = BRIDGE.with_block(Some(own), &lines(&["KEY=\"${RELAY}\""]));
        assert_eq!(
            applied,
            format!("{own}\n{START}\nKEY=\"${{RELAY}}\"\n{BLOCK_END}\n")
        );
        assert!(BRIDGE.has_block(Some(&applied)));
        assert!(BRIDGE.block_has_line(Some(&applied), "KEY=\"${RELAY}\""));
        // A line of the user's is not one of the block's.
        assert!(!BRIDGE.block_has_line(Some(&applied), "KEY=k"));
        assert!(!BRIDGE.has_block(Some(own)) && !BRIDGE.has_block(None));
        // Rewritten in place on the next switch, the user's lines kept.
        assert_eq!(
            BRIDGE.with_block(Some(&applied), &lines(&["URL='u'"])),
            format!("{own}\n{START}\nURL='u'\n{BLOCK_END}\n")
        );
        // Removed with its blank line; a blank line of the user's stays.
        assert_eq!(BRIDGE.without_block(&applied), own);
        let spaced = format!("{own}\n");
        let block = BRIDGE.with_block(Some(&spaced), &lines(&["A=1"]));
        assert_eq!(BRIDGE.without_block(&block), spaced);
        // A `.env` of Copool's block alone starts with it.
        assert!(BRIDGE.with_block(None, &lines(&["A=1"])).starts_with(START));
        // The file's newlines, after a last line without one.
        assert_eq!(
            BRIDGE.with_block(Some("A=1\r\nB=2"), &lines(&["C=3"])),
            format!("A=1\r\nB=2\r\n\r\n{START}\r\nC=3\r\n{BLOCK_END}\r\n")
        );
        // A CRLF file's block is recognized and removed as an LF one is.
        let crlf = BRIDGE.with_block(Some("A=1\r\n"), &lines(&["C=3"]));
        assert!(BRIDGE.has_block(Some(&crlf)) && BRIDGE.block_has_line(Some(&crlf), "C=3"));
        assert_eq!(BRIDGE.without_block(&crlf), "A=1\r\n");
        // Another bridge's block is not this one's.
        let other = Bridge {
            block_start: "# >>> Copool: another >>>",
            ..BRIDGE
        };
        assert!(!other.has_block(Some(&applied)));
        assert_eq!(other.without_block(&applied), applied);
    }

    #[test]
    fn removing_takes_the_block_and_puts_it_back_on_undo() {
        let dir = tempfile::tempdir().unwrap();
        let env = dir.path().join("tool/.env");
        // No file or no block: nothing changes.
        BRIDGE.remove(&env).unwrap().undo();
        assert_eq!(read(&env), None);
        write(&env, "A=1\n");
        BRIDGE.remove(&env).unwrap().undo();
        assert_eq!(read(&env).as_deref(), Some("A=1\n"));
        let applied = BRIDGE.with_block(Some("A=1\n"), &lines(&["B=2"]));
        write(&env, &applied);
        let changes = BRIDGE.remove(&env).unwrap();
        assert_eq!(read(&env).as_deref(), Some("A=1\n"));
        changes.undo();
        assert_eq!(read(&env).as_deref(), Some(applied.as_str()));
        // A `.env` of Copool's block alone goes with it.
        write(&env, &BRIDGE.with_block(None, &lines(&["B=2"])));
        BRIDGE.remove(&env).unwrap();
        assert!(!env.exists());
    }

    #[test]
    fn a_startup_file_loads_env_last_once_and_is_put_back() {
        let dir = tempfile::tempdir().unwrap();
        let (rc, env) = (dir.path().join("zshrc"), dir.path().join(".env"));
        let lines = BRIDGE.rc_lines(&env).unwrap();
        assert!(lines.starts_with(&format!("{}\n", BRIDGE.rc_comment)));
        // Absent: created, then removed.
        let before = BRIDGE.load_env_in(&rc, &env).unwrap().unwrap();
        assert_eq!(read(&rc).unwrap(), lines);
        assert_eq!(BRIDGE.load_env_in(&rc, &env).unwrap(), None);
        unload_env_in(&rc, &before);
        assert!(!rc.exists());
        for (text, separator) in [
            ("", ""),
            ("a=1\n", "\n"),
            ("a=1", "\n\n"),
            ("a=1\r\n", "\n"),
        ] {
            write(&rc, text);
            let before = BRIDGE.load_env_in(&rc, &env).unwrap().unwrap();
            assert_eq!(read(&rc).unwrap(), format!("{text}{separator}{lines}"));
            unload_env_in(&rc, &before);
            assert_eq!(read(&rc).unwrap(), text);
        }
        // Edited since, or created by another, it is left as it is now.
        for text in [None, Some("a=1\n")] {
            match text {
                Some(text) => write(&rc, text),
                None => {
                    let _ = std::fs::remove_file(&rc);
                }
            }
            let appended = BRIDGE.load_env_in(&rc, &env).unwrap().unwrap();
            let mut edited = read(&rc).unwrap();
            edited.push_str("export MINE=1\n");
            write(&rc, &edited);
            unload_env_in(&rc, &appended);
            assert_eq!(read(&rc).unwrap(), edited);
        }
        // Loaded earlier but exported over since: loaded last again.
        write(&rc, &format!("{lines}export KEY=x\n"));
        assert!(BRIDGE.load_env_in(&rc, &env).unwrap().is_some());
        assert!(read(&rc).unwrap().ends_with(&lines));
        // Never a path a shell would take anywhere else, or break the line.
        for env in [".env", "/h/it's/.env", "/h/a\rb/.env", "/h/a\nb/.env"] {
            assert!(BRIDGE.rc_lines(Path::new(env)).is_err(), "{env:?}");
        }
    }

    /// The line loads `.env` in a shell started for the user, but not in
    /// one asked what the user exports.
    #[cfg(unix)]
    #[test]
    fn the_startup_line_loads_env_except_when_asked_for_the_users_own_exports() {
        let dir = tempfile::tempdir().unwrap();
        let env = dir.path().join(".env");
        write(
            &env,
            &BRIDGE.with_block(None, &lines(&["KEY=\"${RELAY_A}\""])),
        );
        let run = |own_only: bool| {
            let mut shell = std::process::Command::new("/bin/sh");
            shell
                .arg("-c")
                .arg(format!(
                    "{}printf %s \"$KEY\"",
                    BRIDGE.rc_lines(&env).unwrap()
                ))
                .env("RELAY_A", "key-a")
                .env_remove("KEY");
            if own_only {
                shell.env(crate::proxy_integration::OWN_EXPORTS_ONLY, "1");
            } else {
                shell.env_remove(crate::proxy_integration::OWN_EXPORTS_ONLY);
            }
            String::from_utf8(shell.output().unwrap().stdout).unwrap()
        };
        assert_eq!(run(false), "key-a");
        assert_eq!(run(true), "");
    }

    #[tokio::test]
    async fn putting_checks_a_new_shell_and_loads_env_from_the_startup_file_when_needed() {
        let dir = tempfile::tempdir().unwrap();
        let (env, rc) = (dir.path().join(".env"), dir.path().join(".zshrc"));
        let own = "OWN=1\r\n";
        write(&env, own);
        write(&rc, "alias ll='ls -l'\n");
        let block = lines(&["KEY=\"${RELAY}\"", "URL='https://r/gw'"]);
        let expect = [
            Expect::Same("KEY", "RELAY"),
            Expect::Address("URL", "https://r/gw"),
        ];
        let loads = Arc::new(Mutex::new(false));
        let environment = shell(&[("RELAY", "sk")], &env, loads.clone());

        // The shell loads `.env` already: nothing else changes.
        *loads.lock().unwrap() = true;
        let changes = BRIDGE
            .put(
                &env,
                &block,
                Some(rc.clone()),
                environment.clone(),
                &expect,
                "not loaded",
            )
            .await
            .unwrap();
        assert_eq!(
            read(&env).unwrap(),
            format!(
                "{own}\r\n{START}\r\nKEY=\"${{RELAY}}\"\r\nURL='https://r/gw'\r\n{BLOCK_END}\r\n"
            )
        );
        assert_eq!(read(&rc).unwrap(), "alias ll='ls -l'\n");
        changes.undo();
        assert_eq!(read(&env).unwrap(), own);

        // It does not: the startup file loads it, last.
        *loads.lock().unwrap() = false;
        let flips = loads.clone();
        let flipping: Environment = Arc::new(move |names: &[&str], now: bool| {
            let values = environment(names, now);
            *flips.lock().unwrap() = true;
            values
        });
        let changes = BRIDGE
            .put(
                &env,
                &block,
                Some(rc.clone()),
                flipping,
                &expect,
                "not loaded",
            )
            .await
            .unwrap();
        let loaded = read(&rc).unwrap();
        assert!(
            loaded.starts_with("alias ll='ls -l'\n\n# Copool:"),
            "{loaded}"
        );
        changes.undo();
        assert_eq!(read(&rc).unwrap(), "alias ll='ls -l'\n");
        assert_eq!(read(&env).unwrap(), own);

        // Even then not, or with no startup file to load it from: all back.
        *loads.lock().unwrap() = false;
        let never = shell(&[("RELAY", "sk")], &env, Arc::new(Mutex::new(false)));
        for rc_file in [Some(rc.clone()), None] {
            assert_eq!(
                BRIDGE
                    .put(&env, &block, rc_file, never.clone(), &expect, "not loaded")
                    .await
                    .unwrap_err(),
                "not loaded"
            );
            assert_eq!(read(&env).unwrap(), own);
            assert_eq!(read(&rc).unwrap(), "alias ll='ls -l'\n");
        }

        // Another address, or a key the variable does not hold, is not loaded.
        let other = shell(
            &[("RELAY", "sk"), ("KEY", "else")],
            &env,
            Arc::new(Mutex::new(false)),
        );
        for expect in [
            [
                Expect::Same("KEY", "RELAY"),
                Expect::Address("URL", "https://r/gw"),
            ],
            [
                Expect::Same("KEY", "MISSING"),
                Expect::Address("URL", "https://r/gw"),
            ],
        ] {
            assert!(!exports(other.clone(), &expect).await.unwrap());
        }
        let exact = shell(
            &[("K", "v"), ("V", "v"), ("U", " https://r/gw/ ")],
            &env,
            loads,
        );
        assert!(exports(
            exact,
            &[Expect::Same("K", "V"), Expect::Address("U", "https://r/gw")]
        )
        .await
        .unwrap());
    }
}
