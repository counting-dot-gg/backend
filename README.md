# counting.gg backend

[![ci](https://github.com/counting-dot-gg/backend/actions/workflows/ci.yml/badge.svg)](https://github.com/counting-dot-gg/backend/actions/workflows/ci.yml)
[![licence](https://img.shields.io/badge/licence-Apache--2.0-blue)](LICENSE)

This package contains (most) of the backend code running [counting.gg](https://counting.gg). It's divided into: 
* an API service
* a websocket service
* a stats service
* a bot service
which run independently from one another, but still interact with one another. 

The original backend components were written in TypeScript, but were migrated to Rust.
To maintain parity, some TypeScript implementations were ported over.

The counting.gg backend uses a private repository which patches functionality from this
repo, for things like secret achievements and to obfuscate abuse detection strategies.

Contributions are encouraged.

# Setup

Trying to make this beginner friendly! Back in the good old days I had no idea how
to set stuff up and wished for copy-paste instructions or videos, so I'll provide both.

Warning: AI slop below! Or is it an AI gem...?

- [Agentic coding](#agentic-coding)
- [Windows](#windows)
- [Linux (Ubuntu / Debian)](#linux-ubuntu--debian)
- [Mac](#mac)
- [Using it](#using-it)
- [If something goes wrong](#if-something-goes-wrong)

## Agentic coding

[Claude Code](https://claude.com/claude-code) and
[Codex](https://developers.openai.com/codex) can install everything and start
the backend for you. This repo includes instructions they read automatically.

1. Install one of them, following its install page.
2. Open a terminal (see step 1 for [Windows](#1-open-a-terminal),
   [Linux](#1-open-a-terminal-1) or [Mac](#1-open-a-terminal-2)) and start it
   by typing `claude` or `codex`.
3. Paste this and follow along, approving each step it asks about:

   ```
   Clone https://github.com/counting-dot-gg/backend, set up the backend on my
   computer by following its README, start it, and post a test count.
   ```

It will also run the checks and explain errors for you when you make a
change. Prefer to do it yourself? Follow the steps for your system below.

## Windows

https://github.com/user-attachments/assets/81a10bbe-2065-451f-8716-cfa1b8fc6aab

### 1. Open a terminal

Press the **Windows key**, type `PowerShell`, and click **Windows
PowerShell**. A window with a blinking cursor opens. That's the terminal:
you type a command, press **Enter**, and wait for it to finish.

### 2. Install the tools

`winget` is Windows' built-in app installer. Copy each line below into the terminal, press **Enter**, and wait for it to
finish before the next one. Click **Yes** if Windows asks for permission.

```
winget install --id Git.Git -e
winget install --id Rustlang.Rustup -e
winget install --id Microsoft.VisualStudio.2022.BuildTools -e --override "--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
winget install --id Docker.DockerDesktop -e
```

- **Git** downloads the code.
- **Rust** builds it. On Windows it also needs Microsoft's C++ build tools
  (the long third line installs just those, without the rest of Visual
  Studio).
- **Docker** runs the database.

Then **restart your computer**. After restarting, open **Docker Desktop**
from the Start menu once and let it finish starting (the whale icon by the
clock stops animating). You can skip its sign-in.

### 3. Download the code

Open PowerShell again and run:

```
cd ~\Documents
git clone https://github.com/counting-dot-gg/backend.git
cd backend
copy .env.example .env
```

### 4. Start the database

```
docker compose up -d mysql redis
```

### 5. Build the backend

```
cargo build --release -p cgg-socket -p cgg-harness
```

The first build takes 5–15 minutes. Later builds are much faster.

### 6. Run it

```
.\target\release\cgg-harness.exe dev
```

When you see `counting.gg backend running`, it's up. Leave this window open
and go to [Using it](#using-it). On Windows, start commands with
`.\target\release\cgg-harness.exe` wherever that section says
`target/release/cgg-harness`.

## Linux (Ubuntu / Debian)



https://github.com/user-attachments/assets/b3be27bf-227e-4f73-b28d-5d14e8cf341f



### 1. Open a terminal

Press **Ctrl + Alt + T**, or open **Terminal** from your apps.

### 2. Install the tools

```
sudo apt update
sudo apt install -y git build-essential curl docker.io docker-compose-v2
sudo usermod -aG docker $USER
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
```

Type your password if asked (nothing shows while you type; that's normal).
Then **log out and back in**, so the Docker and Rust changes take effect.

### 3. Download the code

```
cd ~
git clone https://github.com/counting-dot-gg/backend.git
cd backend
cp .env.example .env
```

### 4. Start the database

```
docker compose up -d mysql redis
```

### 5. Build the backend

```
cargo build --release -p cgg-socket -p cgg-harness
```

The first build takes 5–15 minutes.

### 6. Run it

```
target/release/cgg-harness dev
```

When you see `counting.gg backend running`, it's up. Leave this terminal
open and go to [Using it](#using-it).

## Mac



https://github.com/user-attachments/assets/bb033117-6f1c-4603-8beb-87b4b095baf5



### 1. Open a terminal

Press **Cmd + Space**, type `Terminal`, and press **Enter**.

### 2. Install the tools

```
xcode-select --install
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
```

Click **Install** in the window that pops up for the first line. Then install
[Docker Desktop for Mac](https://www.docker.com/products/docker-desktop/),
open it once, and let it finish starting. Close and reopen Terminal.

### 3 to 6

Same as [Linux, step 3](#3-download-the-code-1) onwards.

## Using it

Open a **second** terminal (leave the first one running the backend), go to
the code folder, and count:

```
cd backend
target/release/cgg-harness say main 1
target/release/cgg-harness say --as 1 main 2
target/release/cgg-harness say --as 1 main 3
```

You'll see each post and whether it counted:

```
harness_0: 1  valid, count #1
harness_1: 2  valid, count #2
harness_1: 3  not valid
```

`--as 0` to `--as 4` picks which of the five demo accounts posts. The third
one isn't valid because the same account can't count twice in a row in
`main`. The `double_counting` thread allows it.

- **Stop the backend:** press **Ctrl + C** in its terminal.
- **Start it again, keeping your counts:** `target/release/cgg-harness dev --keep`
- **Start fresh:** `target/release/cgg-harness dev`
- **Stop the database:** `docker compose stop`

### Check a change

Before sending a change, run:

```
cargo test --release --workspace
target/release/cgg-harness test
```

The second line starts the real socket and checks counting still works:
counts in order, wrong counts and double posts rejected, recovery after a
crash. Every pull request runs both, plus a speed comparison. See
[CONTRIBUTING.md](CONTRIBUTING.md).

## If something goes wrong

| You see | Do this |
|---|---|
| `cargo` / `git` / `docker` is not recognized, or command not found | Close the terminal and open a new one. If that doesn't help, restart the computer. |
| `connect to MySQL` or `connect to Redis` … refused | The database isn't running. Open Docker Desktop, wait for it to start, then run `docker compose up -d mysql redis` again. |
| `port is already allocated` when starting the database | Something else on your computer already uses port 3306 or 6379, such as your own MySQL or Redis. Stop it, or ask for help. |
| `linker link.exe not found` (Windows) | The Visual Studio build tools didn't install. Run that `winget` line again, then restart. |
| `is cgg-harness dev running on port 8805?` | Start the backend first (step 6), in a separate terminal. |
| The build stops with an error | Copy the last 20 lines and open an issue on GitHub. |

The backend only ever creates databases named `cgg_harness…` and only talks
to the database on your own computer.

There's also an all-in-Docker option, no Rust needed: `docker compose up`
builds and runs everything, and
`docker compose exec backend target/release/cgg-harness say main 1` counts.
It's slower to build and harder to change code with.

## Licence

Apache License 2.0. See [LICENSE](LICENSE).
