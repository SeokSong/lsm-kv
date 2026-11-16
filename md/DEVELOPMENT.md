# Development

**Status:** All building, testing, linting, and benchmarking happens in a Linux container (Nov 16). Linux is the target, the Mac only runs the editor and Docker.

## Why a container

- The store targets Linux: `fdatasync`, Linux file system behavior, the Linux page cache
- The Mac's macOS is not the target, so tests passing there don't prove much
- One pinned toolchain (`rust:1.100-bookworm`) for every build and benchmark run

## Files

| File | What |
|---|---|
| `Dockerfile` | Rust 1.100 on Debian bookworm, rustfmt + clippy, build output in `/target` |
| `compose.yaml` | The `dev` service: repo mounted at `/workspace`, volumes for the cargo registry and `/target` |
| `.devcontainer/devcontainer.json` | VS Code opens the repo inside the `dev` service, with rust-analyzer |
| `.dockerignore` | Keeps `target/` and `.git/` out of the image build |

- Build output goes to a Docker volume, not `target/` in the repo: file I/O through the macOS bind mount is slow, and Mac and Linux builds would overwrite each other
- Source edits on the Mac show up in the container right away (bind mount)

## Commands

Run from the repo root (`~/dev/lsm-tree-key-value-store/lsm-kv`):

| Task | Command |
|---|---|
| Build the image (first time, after a `Dockerfile` change) | `docker compose build` |
| Tests | `docker compose run --rm dev cargo test` |
| Format check | `docker compose run --rm dev cargo fmt --check` |
| Lint | `docker compose run --rm dev cargo clippy --all-targets -- -D warnings` |
| All benchmarks | `docker compose run --rm dev cargo bench` |
| One benchmark, short run | `docker compose run --rm dev cargo bench --bench sstable -- --warm-up-time 1 --measurement-time 3` |
| A shell in the container | `docker compose run --rm dev bash` |
| Everything CI checks (fmt, clippy, tests, benchmarks build) | `scripts/check.sh`, or `scripts/check.sh merge` for only tests matching `merge` |

- Criterion's HTML reports land in the `target` volume. To keep one, copy it out: `docker compose run --rm dev bash -c "cp -r /target/criterion /workspace/criterion-report"` (not in `.gitignore`, so don't commit it)

## Benchmarks on Linux x86

- `.github/workflows/bench.yml` runs `cargo bench` on a GitHub Actions `ubuntu-latest` runner, only when started by hand
- Start: Actions tab -> Bench -> Run workflow (optional: seconds per case, one benchmark), or `gh workflow run bench.yml`
- Watch: `gh run watch`, or the run page on GitHub
- Results: the run's summary page, or `gh run download <run id>` for `bench.txt`, `machine.txt`, and the criterion HTML report
- Runners differ (CPU, disk), so run it several times and compare only runs on the same CPU. Results in [BENCHMARKS.md](BENCHMARKS.md)

## VS Code

- Install the Dev Containers extension
- Command palette: "Dev Containers: Reopen in Container"
- rust-analyzer, the terminal, and tasks then all run on Linux inside the container

## Limits

- The container runs in Docker Desktop's Linux VM on the M3 Pro: Linux kernel, real cores, but a virtual disk
- fsync and cold-cache numbers from the container can't be used, see [BENCHMARKS.md](BENCHMARKS.md). Those need Linux on real hardware
- It's aarch64. x86 checks (CRC) still need other hardware. CI runs on x86 ubuntu

## See also

[BENCHMARKS.md](BENCHMARKS.md) · [OVERVIEW.md](OVERVIEW.md)
