use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;
use std::thread;

use serde::Deserialize;

use crate::Event;
use crate::process::spawn_group;

#[derive(Deserialize)]
struct Message {
    reason: String,
    #[serde(default)]
    executable: Option<PathBuf>,
    #[serde(default)]
    target: Option<Target>,
}

#[derive(Deserialize)]
struct Target {
    name: String,
    kind: Vec<String>,
}

/// Starts `cargo build` and returns its process ID. A thread sends `Event::BuildDone`
/// with the path of the built binary when cargo stops.
pub fn start(id: u64, cargo_args: &[String], tx: Sender<Event>) -> std::io::Result<u32> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut cmd = Command::new(cargo);
    // Cargo writes the diagnostics to stderr as usual, and the JSON messages to stdout.
    cmd.args(["build", "--message-format=json-render-diagnostics"])
        .args(cargo_args)
        .stdout(Stdio::piped());
    let mut child = spawn_group(&mut cmd)?;
    let pid = child.id();
    let stdout = child.stdout.take().expect("stdout is piped");
    thread::spawn(move || {
        let found = executables(stdout);
        let result = match child.wait() {
            Ok(status) if status.success() => pick(found),
            Ok(_) => Err("the build failed".into()),
            Err(e) => Err(format!("cannot wait for cargo: {e}")),
        };
        let _ = tx.send(Event::BuildDone { id, result });
    });
    Ok(pid)
}

/// Reads the cargo JSON messages and returns the binaries that cargo built.
fn executables(out: impl Read) -> Vec<(String, PathBuf)> {
    let mut found = Vec::new();
    for line in BufReader::new(out).lines() {
        let Ok(line) = line else { break };
        // Most lines are not artifacts. This check is much faster than a JSON parse.
        if !line.contains("\"compiler-artifact\"") {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<Message>(&line) else { continue };
        if let (Some(exe), Some(target)) = (msg.executable, msg.target)
            && msg.reason == "compiler-artifact"
            && target.kind.iter().any(|k| k == "bin" || k == "example")
        {
            found.push((target.name, exe));
        }
    }
    found
}

fn pick(found: Vec<(String, PathBuf)>) -> Result<PathBuf, String> {
    match found.len() {
        1 => Ok(found.into_iter().next().unwrap().1),
        0 => Err("cargo built no binary. Use --bin, --example, or -x".into()),
        _ => {
            let names: Vec<_> = found.iter().map(|(name, _)| name.as_str()).collect();
            Err(format!("cargo built more than one binary ({}). Use --bin <NAME>", names.join(", ")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUTPUT: &str = r#"{"reason":"compiler-artifact","target":{"kind":["lib"],"name":"core"},"executable":null}
{"reason":"compiler-artifact","target":{"kind":["custom-build"],"name":"build-script-build"},"executable":null}
{"reason":"compiler-artifact","target":{"kind":["bin"],"name":"app"},"executable":"/p/target/debug/app"}
{"reason":"build-finished","success":true}
"#;

    #[test]
    fn finds_the_binary() {
        let found = executables(OUTPUT.as_bytes());
        assert_eq!(pick(found), Ok(PathBuf::from("/p/target/debug/app")));
    }

    #[test]
    fn asks_for_a_choice_when_there_are_many_binaries() {
        let two = vec![("a".into(), "/a".into()), ("b".into(), "/b".into())];
        assert!(pick(two).unwrap_err().contains("a, b"));
    }
}
