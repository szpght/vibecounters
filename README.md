# Vibe Counters

Single-binary web app for creating counters that track time to/from a target datetime.

## Features
- CRUD for counters via REST API (`/api/counters`)
- Embedded static UI (vanilla HTML/CSS/JS)
- Responsive layout for desktop/mobile
- JSON persistence with atomic writes (temp file + rename)

## Run
```powershell
# build and run
cargo run

# optionally set data file path
$env:COUNTERS_FILE="D:/data/counters.json"; cargo run
```

Open http://localhost:3000 in your browser.

