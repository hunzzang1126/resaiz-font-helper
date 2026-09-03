# resaiz Font Helper

A tiny program that lets the resaiz editor (https://resaiz.vercel.app) use the fonts installed on your computer, including fonts activated through Adobe Fonts. Browsers no longer expose the installed font list to websites, so this helper reads it locally and serves it to resaiz on `127.0.0.1` only.

컴퓨터에 설치된 폰트(Adobe Fonts로 활성화한 폰트 포함)를 resaiz 에디터에서 그대로 쓰게 해주는 작은 프로그램입니다. 브라우저가 사이트에 설치 폰트 목록을 더 이상 넘겨주지 않기 때문에, 이 프로그램이 로컬에서 목록을 읽어 `127.0.0.1`로만 전달합니다.

## Install

Download the zip for your platform from the Releases page, unzip, then:

macOS

```
bash install-mac.sh
```

The binary is not notarized yet, so the script removes the quarantine flag itself. If you run the binary by hand instead, right click it and choose Open the first time.

Windows (PowerShell)

```
.\install-win.ps1
```

Both scripts copy the binary to a user folder and start it at login. Uninstall with `uninstall-mac.sh` or `uninstall-win.ps1`.

## What it does

- Scans the system, user and Adobe Fonts directories on start and every 60 seconds when a folder changes.
- Serves `GET /v1/health`, `GET /v1/fonts` and `GET /v1/font/{id}` on `http://127.0.0.1:57731` (falls back to 57732 to 57740).
- Answers only browser origins belonging to resaiz. Other sites get 403. File paths never leave the process.
- No telemetry, no network access other than the local port.

## Build from source

```
cargo build --release
./target/release/resaiz-font-helper
```

`--once` prints the font list as JSON and exits. `--port N` pins the port.

## License

MIT
