# rwatch

`rwatch` watches a Rust project. When a file changes, it rebuilds the project and restarts the app. It works like `nodemon` or `tsx watch`.

## Install

```sh
cargo install --path .
```

## Use

```sh
rwatch                          # build and run the binary in the current project
rwatch -p app --bin agent-desk  # select a package and a binary in a workspace
rwatch -- --port 8080           # send arguments to the app
rwatch -x "cargo test"          # run a shell command on each change
rwatch -e rs,toml,html -i "assets/gen/"  # change the watched extensions and ignored paths
```

Type `rs` and Enter to restart manually. Press Ctrl-C to stop. Press Ctrl-C two times to stop at once.

## Why it is fast

- It uses the native file events of the operating system (FSEvents on macOS, inotify on Linux).
- It waits 100 ms (`--delay`) for more changes, so one save of many files causes one build.
- It starts the built binary directly. It does not use `cargo run`, so there is no second Cargo step.
- The old app continues to run while the new build runs. A failed build does not stop the old app.
- A new change stops an old build that is out of date.

## How it stops the app

`rwatch` starts each app in a new process group. To restart, it sends SIGTERM to the group. If the app does not stop in 3000 ms (`--grace`), it sends SIGKILL. Then it stops all child processes that remain, so ports become free for the new app.

`rwatch` supports macOS and Linux.
