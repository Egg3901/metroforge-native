# MetroForge Native 1.0.0 release notes draft

Use this as the body seed for the GitHub-generated 1.0.0 release.

## Highlights

- Rust sim consolidation is complete. Desktop runtime is now embedded-only and no longer depends on an external sidecar process.
- Real-city static buildings (`msgType=5`) are emitted from `mf-net`, including embedded flagship datasets and on-demand city-data loading.
- Data-driven scenarios, progression, replay command logs, and save/load now run through the Rust transport path.
- Host transient parity improvements landed: cohort-demand UI payload, demand/traffic overlay emission, and frame agent-particle output.

## Packaging and CI

- Release packaging no longer stages or installs sidecar binaries.
- CI/release workflows were updated to remove sidecar compile and sidecar-only smoke/recovery jobs.
- Windows installer payload now contains only the desktop executable and assets.

## Validation

- Gates: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`.
- Embedded visual verification executed with `MF_AUTOSTART=nyc` and verify screenshots captured (`menu`, `default`, `transit`, `street`, `subway`, `potato`, `pause`).

## Notes for tag owner

- Branch is prepared for `v1.0.0` tagging after final owner review.
- Tag creation/push intentionally left to owner sign-off.
