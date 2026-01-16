
# BTWd — Bumblebee Trusts Wikipedia Daemon

Wake-word voice assistant daemon for Arch Linux.
Uses wake word detection (Porcupine) with local audio capture/VAD, and then routes speech to either an allow-listed command executor or an LLM.
Commands are executed only if explicitly defined in `commands.json`.

## Features

- Wake word detection (Porcupine)
- Local audio capture + VAD
- Intent routing with strict command allow-list
- LLM responses (Groq / Mistral)
- Web fallback via Tavily (only when required)
- OSD / notification support 
- Safe command execution with confirmation

## Installation (Arch Linux – step by step)

### All files available in Releases Tab

### 4.1 System dependencies (pacman)

Install build + runtime deps:

```zsh
sudo pacman -S --needed \
	git \
	rust \
	cargo \
	python \
	python-virtualenv \
	alsa-lib
```

Audio backends depend on your system:

- PipeWire users: ensure `pipewire` + `pipewire-pulse` are installed/running.
- ALSA-only users: ensure your ALSA device is working.


### 4.2 Clone & build

```zsh
git clone https://github.com/Bumblebee-3/BTW-daemon.git
cd BTW-daemon/btwd

cargo build --release
```

The binary will be at:

- `target/release/btwd`

If you want it on your PATH:

```zsh
install -Dm755 target/release/btwd "$HOME/.local/bin/btwd"
```

### 4.3 Porcupine setup

BTWd expects the Porcupine shared library and model files to be available locally.

1) Download Porcupine 4.0 from Picovoice and extract it.
2) Place the following files somewhere stable (example layout below):

```text
$HOME/.local/lib/libpv_porcupine.so
$HOME/.local/share/porcupine/porcupine_params.pv
$HOME/.local/share/porcupine/btw.ppn
```

This repo already contains a wake-word file `btw.ppn` at the project root; you can use that path directly in config.

Make sure the loader can find `libpv_porcupine.so`.
Example (manual run):

```zsh
export LD_LIBRARY_PATH="$HOME/.local/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
```

### 4.4 Python ML environment

The ASR worker is a small Python process in `ml/btw_ml.py`.

```zsh
cd btwd

python -m venv .venv
source .venv/bin/activate

python -m pip install --upgrade pip
python -m pip install groq numpy
```

## Configuration

### 5.1 `config.toml` (example)

Start from `example.config.toml` and adjust paths.

```toml
# Identity (optional)
name = "btwd"
description = "Wake word voice assistant daemon"

[wake_word]
# Wake-word model (Porcupine)
ppn_path = "/home/you/.local/share/porcupine/btw.ppn"
model_path = "/home/you/.local/share/porcupine/porcupine_params.pv"
device = "cpu"
sensitivity = 0.6

[speech]
# VAD/utterance control
silence_threshold = 0.01        # normalized RMS (0.0..1.0)
silence_duration_ms = 700       # continuous silence required
max_utterance_seconds = 30      # hard safety cap

[execution]
# Command confirmation safety
confirmation_timeout_seconds = 10
dry_run = false

[ui]
# Notifications (works with swaync)
listening_notification = true   # toast on wake
osd = true                      # allow text notifications
osd_timeout_ms = 2000           # auto-dismiss (ms)

[speech_output]
# TTS output (LLM provider dependent)
enabled = true
provider = "groq"              # "groq" or "mistral"
voice = "alloy"
format = "wav"
rate = 1.0

[search]
# Web fallback via Tavily
enabled = true
timeout_ms = 3500
country = "india"              # optional (e.g. "india", "us")

[llm]
# LLM backend used for intent + answering
provider = "mistral"
```

### 5.2 `.env` (example)

Create `.env` in the project root (or export these in your service environment).
Start from `example.env`:

```dotenv
# Required for Porcupine wake word
PICOVOICE_ACCESS_KEY=pk-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx

# Required for ASR (Groq Whisper) and TTS/summarization
GROQ_API_KEY=gsk_yyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyy
MISTRAL_API_KEY=abcdefghijklmnopqrstuvwxyz123456


# enables read-only web answers via Tavily
TAVILY_API_KEY=tttttttttttttttttttttttttttttttt
```

### 5.3 `commands.json` (example)

Commands are an allow-list: BTWd will only execute commands that exist in your `commands.json`.

- `dangerous: true` commands trigger a strict confirmation flow.
- Templates use simple placeholders like `{value}` / `{delta}`.

Start from `example.commands.json`:

```json
[
	{
		"id": "lock_screen",
		"category": "system",
		"description": "Lock the current user session",
		"examples": ["lock my computer", "lock the screen"],
		"dangerous": false,
		"parameters": {},
		"shell_command_template": "loginctl lock-session"
	},
	{
		"id": "system_shutdown",
		"category": "power",
		"description": "Shut down the system",
		"examples": ["shut down", "power off"],
		"dangerous": true,
		"parameters": {},
		"shell_command_template": "systemctl poweroff"
	}
]
```

## Running

Manual run (recommended while iterating):

```zsh
export LD_LIBRARY_PATH="$HOME/.local/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
set -a
source ./.env
set +a

# BTWd uses XDG config paths by default:
#   ~/.config/btw/config.toml
#   ~/.config/btw/commands.json
#   ~/.config/btw/.env

mkdir -p ~/.config/btw
cp -n ./example.config.toml ~/.config/btw/config.toml
cp -n ./example.commands.json ~/.config/btw/commands.json
cp -n ./example.env ~/.config/btw/.env

./target/release/btwd
```

Systemd user service (example):

- The repo includes `btw.service` (adjust paths to your user/home).
- Optional drop-in for TTS config: `systemd/btw.service.d/override-tts.conf`.

## Known limitations

- Requires explicit command definitions (`commands.json`); unknown commands are not executed.

## Credits/APIs required

- Picovoice (Porcupine)
- Groq
- Mistral
- Tavily

## TODO

[ ] Add plugins integration

  [ ] Google Calendar

  [ ] Gmail
