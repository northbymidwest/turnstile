#!/bin/bash
# Bundles and launches. Use this rather than `cargo run` when checking
# anything that needs a bundle: localization, the icon, or the About panel.
set -euo pipefail
./scripts/bundle.sh
open dist/Turnstile.app
