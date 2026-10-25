# Backsave for Revit

A free web tool that saves Revit files back to an older Revit version. Pick a file, choose the target year, press
**Downgrade**, and download the result. The web app and the server are open source (MIT).

## Supported files

- `.rvt` (project), `.rte` (project template), `.rfa` (family), `.rft` (family template)
- Up to 1 GiB per file
- Source and target versions between Revit 2023 and 2027, limited to the version pairs the conversion engine reports
  through `capabilities`

The conversion engine is closed source and is not part of this repository. The server runs it as an external
command-line program.

## Privacy

- Your file is sent only when you press **Downgrade**. Choosing a file only reads its version in your browser.
- No tracking cookies and no analytics.
- Uploaded files are not shared. Sending a failure report is opt-in and asks for your consent each time.
- Server logs never contain file names or file content.
- To have a report deleted early, email phancongvuduc@gmail.com or open an issue at
  https://github.com/PhanCongVuDuc/backsave-revit/issues with your report code (the code alone identifies nothing
  about you or your file).

| What | How long it is kept |
| --- | --- |
| Upload not finished | Removed after 1 hour without new data (6 hours at most) |
| Cancelled conversion | Deleted immediately |
| Input file after a successful conversion | Deleted as soon as the conversion is done |
| Input file after a failed conversion | 15 minutes, so you can choose to send a report |
| Converted file and the list of dropped elements | 1 hour |
| Failure report (only if you send it) | 30 days, within a 20 GiB total cap (oldest removed first) |
| Everything in `jobs/` | Wiped whenever the server starts |

The rules are in [`server/src/cleanup.rs`](server/src/cleanup.rs) and [`server/src/reports.rs`](server/src/reports.rs).

## Architecture

- `server/` (`backsave-server`): Rust and axum. Resumable uploads (tus), a job queue, per-IP limits, retention, and
  static hosting of the web app.
- `engine-stub/`: a small stand-in for the conversion engine. It copies files and can simulate refusals and failures,
  which is what the tests and the demo image use.
- `web/`: Vue 3 single-page app (English and Vietnamese). It reads the file version in the browser, uploads, shows
  progress and results.
- The server talks to the engine only through its command line: `--version`, `capabilities --json`,
  `probe <file> --json` and `downgrade <in> --to <year> --out <out> --log <log>`.

## Run locally

Requirements: Rust (stable) and Node 22 or newer.

```sh
# Build the web app and the server with the engine stub
(cd web && npm ci && npm run build)
cargo build -p backsave-server -p engine-stub

# Start the server (the stub stands in for the engine)
BACKSAVE_ENGINE=target/debug/engine-stub \
BACKSAVE_DATA=./data \
BACKSAVE_WEB_DIR=web/dist \
cargo run -p backsave-server
# open http://127.0.0.1:8080
```

For front-end development run `npm run dev` in `web/` alongside the server.

Tests:

```sh
cargo test --workspace
cd web
npm test                          # unit tests
npx playwright install chromium   # once, before the first e2e run
npm run test:e2e                  # builds and starts the real server on the stub
```

### Server configuration

| Variable | Default | Meaning |
| --- | --- | --- |
| `BACKSAVE_ENGINE` | required | Path to the conversion engine executable |
| `BACKSAVE_DATA` | `./data` | Working and report storage |
| `BACKSAVE_WORKERS` | `1` | Conversions run at the same time |
| `BACKSAVE_LISTEN` | `127.0.0.1:8080` | Address to listen on |
| `BACKSAVE_WEB_DIR` | unset | Directory with the built web app to serve |
| `BACKSAVE_TRUST_PROXY` | `false` | Take the client IP from `X-Forwarded-For` (set it behind a reverse proxy only; `false` in the Docker image, `true` in the compose file) |
| `BACKSAVE_TRUSTED_PROXIES` | unset | Comma-separated IPs or CIDRs; when set, `X-Forwarded-For` is used only for requests from these peers |
| `BACKSAVE_DISK_BUDGET_GIB` | `100` | Disk space all jobs may use together. Each new job reserves twice its size (input and output); when the budget is used up, new jobs get `503 server_full` |

## Self-host with Docker Compose

```sh
BACKSAVE_DOMAIN=example.org docker compose -f deploy/docker-compose.yml up -d --build
```

This starts the server and [Caddy](https://caddyserver.com/), which obtains a TLS certificate for `BACKSAVE_DOMAIN`
(default `localhost`). Conversion data lives in the `data` volume. To try it: `curl -kL http://localhost/api/info`.

The conversion engine is closed source and is not included. The image ships with the stub, which only copies files, so
a fresh deployment does not really convert anything. To use a real engine, mount its executable over `/app/engine`
(see the commented volume in [`deploy/docker-compose.yml`](deploy/docker-compose.yml)). The engine must follow the
command-line contract above and run on Debian 12.

## API overview

| Method and path | Purpose |
| --- | --- |
| `GET /api/info` | Supported version pairs, size limit, output lifetime, server version |
| `POST /api/jobs` | Create a job and get its upload location |
| `OPTIONS`, `HEAD`, `PATCH /api/uploads/{id}` | Resumable upload (tus 1.0.0) |
| `GET /api/jobs/{id}` | Job state, queue position, time estimate, result and error details, and the name, size, kind and versions it was created with |
| `GET /api/jobs/{id}/file` | Download the converted file |
| `GET /api/jobs/{id}/dropped` | Elements the engine could not carry over |
| `POST /api/jobs/{id}/report` | Send a failed file for diagnosis (needs `{"consent": true}`) |
| `DELETE /api/jobs/{id}` | Cancel a job and delete its files |

## Contact

Made by Phan Cong Vu Duc ([LinkedIn](https://www.linkedin.com/in/phancongvuduc/)). Questions and deletion requests:
phancongvuduc@gmail.com or [GitHub issues](https://github.com/PhanCongVuDuc/backsave-revit/issues).

## License

[MIT](LICENSE).

Autodesk and Revit are registered trademarks of Autodesk, Inc. Backsave is an independent project and is not
affiliated with or endorsed by Autodesk.
