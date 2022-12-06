
# Overview
This is the repository for the CS1666 game by team K*rust*y Krabs.

# Usage
Use cargo to build and run it
- `cargo run [--release] -- [arguments]`

# Arguments
- `client --help` to see client arguments
  - `-i <server ip address>`
  - `-p <server port>`
  - `-c <local client port>`
- `server --help` to see server arguments
  - `-f <save file>`
  - `-p <server port>`

# Group Guidelines
1. Get commits in by _at latest_ Tuesday at noon.
The early the better.
This gives the manager enough time to work on merging and making sure it builds.
1. Make your own changes in your own fork.
Then make a PR to the canonical repository.
1. Use `rustfmt` before commit.
Make sure your code actually compiles.
1. Follow Rust naming conventions. Use clippy!
1. Write comments! Especially for code that is unclear.
1. Write tests where appropriate and/or necessary.

# Setup
1. Install Rust via [`rustup`](https://rustup.rs/).
1. Install `ldd` or `zld`, see [Bevy fast compile setup][bevy-fast].
1. Install `rustfmt` via `rustup component add rustfmt`.
1. (Optionally) Install `rust-analyzer`.
In VSCode, it can be found as an extension.
1. Install `pre-commit` (`pip3 install --user pre-commit`) and install the pre-commit git hooks (`pre-commit install`).
1. Set your editor to remove trailing whitespace.

[bevy-fast]: https://bevyengine.org/learn/book/getting-started/setup/#enable-fast-compiles-optional

# Game Controls
## Movement
- A/D: move left/right
- Space: jump

## Mining
- LMB: mine block under cursor
- G: mine block below you

## Debug Camera
- Arrow keys: move free look camera
- R: re-center camera to player

## Network
- O: toggle network loss simulation (drop all packets in and out)
- P: queue a ping to be sent to the server

## Game States
- F1: force-cycle game state (menu -> game -> credits)
- Ctrl+Q: quit game

## Save/Load
- (server saves and loads automatically)
- F2: dump terrain information into the console (lots of junk)
- F2: dump basic chunk information

