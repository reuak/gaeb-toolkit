# Third-party software notices

This repository contains original software of Hawk Vision GmbH licensed under
the MIT License; see [`LICENSE`](LICENSE).

The source repository does not vendor the source code of the dependencies
listed below. Cargo resolves the exact Rust dependency graph recorded in
`Cargo.lock`, and Debian installs the runtime programs while building the
container. Each component remains subject to its own license and copyright
notices.

## Rust dependencies

The following table lists the direct runtime dependencies resolved by the
current `Cargo.lock`. Their transitive dependencies and exact versions are
recorded in `Cargo.lock`; Cargo metadata for the current graph reports license
metadata for every resolved package.

| Component | Version | License |
| --- | ---: | --- |
| anyhow | 1.0.104 | MIT OR Apache-2.0 |
| axum | 0.8.9 | MIT |
| base64 | 0.22.1 | MIT OR Apache-2.0 |
| chrono | 0.4.45 | MIT OR Apache-2.0 |
| clap | 4.6.2 | MIT OR Apache-2.0 |
| hmac | 0.12.1 | MIT OR Apache-2.0 |
| image | 0.25.10 | MIT OR Apache-2.0 |
| lettre | 0.11.22 | MIT |
| printpdf | 0.7.0 | MIT |
| quick-xml | 0.37.5 | MIT |
| regex | 1.13.1 | MIT OR Apache-2.0 |
| reqwest | 0.12.28 | MIT OR Apache-2.0 |
| rusqlite | 0.32.1 | MIT |
| rust_decimal | 1.42.1 | MIT |
| scraper | 0.22.0 | ISC |
| serde | 1.0.229 | MIT OR Apache-2.0 |
| serde_json | 1.0.151 | MIT OR Apache-2.0 |
| sha2 | 0.10.9 | MIT OR Apache-2.0 |
| tempfile | 3.27.0 | MIT OR Apache-2.0 |
| tokio | 1.53.1 | MIT |
| tower-http | 0.6.11 | MIT |
| tracing | 0.1.44 | MIT |
| tracing-subscriber | 0.3.23 | MIT |
| uuid | 1.24.0 | Apache-2.0 OR MIT |
| zip | 2.4.2 | MIT |

Authoritative package metadata, source links and license texts are available
through the package registry entries referenced by Cargo and the upstream
repositories. When distributing a compiled binary or container image, generate
and ship a complete license report for the exact locked transitive graph rather
than relying only on this direct-dependency overview.

## Container runtime components

The production setup additionally uses these separately packaged programs:

| Component | Purpose | License/source |
| --- | --- | --- |
| Tesseract OCR and language data | OCR for scanned PDFs | Apache-2.0; [tesseract-ocr/tesseract](https://github.com/tesseract-ocr/tesseract) and [tesseract-ocr/tessdata](https://github.com/tesseract-ocr/tessdata) |
| Poppler utilities | PDF text extraction, rendering and layout analysis | GPL; [Poppler COPYING](https://gitlab.freedesktop.org/poppler/poppler/-/blob/master/COPYING) |
| Caddy | HTTPS reverse proxy | Apache-2.0; [caddyserver/caddy](https://github.com/caddyserver/caddy) |
| curl and CA certificates | Container health check and TLS trust | Installed as Debian packages under their respective licenses |
| Debian Bookworm | Runtime base image | Individual Debian package licenses |

Inside Debian-based images, package-specific copyright and license information
is conventionally provided below `/usr/share/doc/<package>/copyright`. The
official Caddy image and all Debian packages retain their own upstream notices.

## No endorsement

The names and marks of third-party projects are used only to identify the
software. They do not imply endorsement of Hawk Vision GmbH or the GAEB
Konverter.
