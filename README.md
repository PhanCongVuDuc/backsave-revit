# Backsave for Revit

Save a Revit file back to an older version of Revit, for free, in your browser.

Pick a file, choose the version you need, press **Downgrade**, then download the result.

Works with `.rvt`, `.rte`, `.rfa` and `.rft` files up to 1 GB, Revit 2023 to 2027.

## Is my file safe?

This repository is public so you can check for yourself what happens to your file.

- Nothing is uploaded until you press **Downgrade**. Before that, your browser only reads the Revit version.
- Your file is not shared with anyone.
- No tracking cookies, no analytics.
- File names are never written to the server logs.

How long files stay on the server:

| File | Kept for |
| --- | --- |
| Your original file | Deleted as soon as the conversion is done |
| The converted file | 1 hour, then deleted |
| A cancelled upload | Deleted right away |
| An upload you leave unfinished | Deleted after 1 hour |
| A file that failed to convert | 15 minutes, unless you choose to send it to us |

If a conversion fails, you can choose to send the file so we can fix the problem. It is kept for up to 30 days and only
used for that. To have it deleted sooner, email us with the report code.

The code that deletes files: [`server/src/cleanup.rs`](server/src/cleanup.rs) and
[`server/src/reports.rs`](server/src/reports.rs).

## Run it yourself

```sh
(cd web && npm ci && npm run build)
cargo build -p backsave-server -p engine-stub
BACKSAVE_ENGINE=target/debug/engine-stub BACKSAVE_WEB_DIR=web/dist cargo run -p backsave-server
```

Then open http://127.0.0.1:8080. With Docker: `docker compose -f deploy/docker-compose.yml up -d --build`.

## Contact

Made by Phan Cong Vu Duc ([LinkedIn](https://www.linkedin.com/in/phancongvuduc/)).
Questions or deletion requests: phancongvuduc@gmail.com or
[GitHub issues](https://github.com/PhanCongVuDuc/backsave-revit/issues).

## License

[MIT](LICENSE).

Autodesk and Revit are registered trademarks of Autodesk, Inc. Backsave is an independent project and is not
affiliated with or endorsed by Autodesk.
